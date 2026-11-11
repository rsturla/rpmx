# Testing

## Tests

### External test files

Keep compiler probes, headers and expected output in a package-local directory:

```text
gcc-16/
  package.yaml
  tests/
    arithmetic.c
    threads.c
    cpp/containers.cc
    include/test.h
```

Declare the directory on each test that uses it:

```yaml
tests:
  - name: c-programs
    stage: installed
    files: tests
    script: |
      for source in "$TEST_FILES"/*.c; do
        gcc-16 -I"$TEST_FILES/include" "$source" -o probe
        ./probe
      done
```

`files` names a literal directory relative to the root recipe, including for split recipes.
rpmx copies all regular files recursively and preserves subdirectories.
`TEST_FILES` points to the writable copy inside the test workspace.
The original files do not change when a test writes to its copy.
Invoke shell fixture files with `bash "$TEST_FILES/run.sh"`.
Fixture files do not require executable permission.

The recipe digest and SRPM include the file contents. A fixture change invalidates the recipe lock.
An SRPM rebuild needs no checkout. `--install-dir` and `--test-root` use the files beside the supplied recipe.
Copy the recipe and its fixture directories when you transfer a saved test root.
Use pinned source archives for externally maintained suites. `files` does not fetch URLs or read arbitrary host paths.
Missing directories, empty directories, parent traversal and symlinks are errors.

Each installed test gets a fresh writable workspace and an empty staging directory.
It cannot see the source workspace or staged package files.
Tests with the same dependencies still share prepared root data.
`--keep-workdir` preserves test workspaces for inspection.

### Scripts and command assertions

Use `script` for a test with several shell commands:

```yaml
tests:
  - name: patterns
    stage: installed
    script: |
      printf 'alpha\nbeta\n' > input
      test "$(grep beta input)" = beta
      test "$(grep -c . input)" = 2
```

rpmx runs the script with `bash --noprofile --norc -euo pipefail`.
An unchecked command failure, pipeline failure or unset variable fails the test.
Bash error handling has exceptions for conditions and commands in `&&` or `||` lists.
Use explicit checks for expected failures. Build-stage scripts need Bash in the build root.

Use `expect` to check a command without shell assertions:

```yaml
tests:
  - name: no-match
    stage: installed
    argv: [grep, missing, /dev/null]
    expect:
      exit-code: 1
      stdout: ''
      stderr: ''
```

The default expected exit code is zero. Omit a stream to leave its content unchecked.
Stream comparisons are exact, including trailing newlines. Assertions support text without NUL bytes.
Failures show the expected and actual result. Assertions need Bash and coreutils.
`script` cannot combine with `argv`, `pipeline` or `expect`.
Choose one form: `argv`, `script` or `pipeline`.
Shell `-c` commands in `argv` are errors. Move their shell text to `script`.
Shell steps in test pipelines also use strict Bash options.

### Eternal migration

Update Eternal before changing its pinned rpmx revision:

1. Convert shell `argv` tests in production and bootstrap recipes to `script`.
2. Check expected failures explicitly. Replace status checks with `expect` where possible.
3. Fix early assertions that now fail. Check pipelines that use `grep -q` for SIGPIPE failures.
4. Move compiler inputs and expected results into package-local fixture directories.
5. Declare upstream suite tools on build-stage tests. Remove unused installed-test tools.
6. Run lint and package tests on both supported architectures. Then update `.rpmx-revision`.

Use one package per migration commit. Keep fixtures and test reports in the package review.
Do not update the pin while recipes still use shell `argv` tests.

### Upstream suites and test dependencies

Declare build-test tools on the test:

```yaml
tests:
  - name: upstream
    stage: build
    packages: [dejagnu, diffutils]
    pipeline:
      - uses: make/check
        with:
          dir: build
```

Build-stage test packages join the build root. In a cross build, these tools
run on the host. The target sysroot does not receive them.
The build root includes these packages even with `--no-tests`.
Installed-stage packages join only the disposable test root.
They do not change the build root or the target sysroot.
Reusable steps add their declared tool requirements to the correct root.
Existing `make/check`, `cmake/check` and `meson/test` steps run upstream suites.

### Build and test in one run

```console
rpmx test
rpmx test --test cli
rpmx test --test hello/smoke
rpmx test --test unit --test installed --junit report.xml
rpmx test --json
```

This builds (source, SRPM, RPMs) with tests enabled, then runs the selected
tests with the workspace and the produced RPMs available. `--json` lists the
produced RPMs; use `--junit` for per-test results.

`rpmx build` and `rpmx rebuild` already run every declared test; `rpmx test`
adds selection, JUnit output and `--install-dir`. Pass `rpmx build --no-tests`
to skip them.

`--skip-installed` runs only build-stage tests. That is the useful split for a
cross build: build-stage tests run natively in the build sandbox, while
installed-stage tests need the target architecture to be executable.

### Test existing RPMs

```console
rpmx build
rpmx test --install-dir packages/x86_64
rpmx test --install-dir packages/x86_64 --test installed
rpmx test --install-dir packages/x86_64 --test hello/smoke
rpmx test --install-dir packages/x86_64 --json --junit report.xml
```

- Reads `*.rpm` from the directory, ignores `*.src.rpm`.
- Skips the build entirely.
- Runs installed-stage tests only. With no `--test`, build-stage tests are
  skipped; naming one is an error.
- Needs a root lock, not an assembled `--root`; the test root is prepared from
  the lock and the supplied RPMs. For a cross package the target sysroot lock
  is used, so the test root is the target architecture.
- `--test-root-lock FILE` pins the test root instead of resolving one.
- `--test-root DIR` runs against a saved test root instead of preparing one.
  The root is self-contained: no build root, no RPMs, no cache, no profile and
  no network. `DIR.lock.yaml` beside it is read automatically.
- JSON: `{schema, ok, command: "test", tests: [{name, stage, ok, duration_ms,
  message}]}`. An output-level test uses its qualified name, `output/test`.
- Exit 6 on failure.

Test roots are disposable and use real dependency checking. Tests with the
same output and `packages` set share a root; a different output or set gets its
own root. A recipe-level installed test sees every output. An output-level
installed test sees that output plus the sibling outputs that provide its
runtime dependencies, so a missing `Requires` between outputs is not hidden by
installing the whole output set. The selected RPMs are resolved like published
packages: a local repository holds them while the test root is resolved, so
their requirements, including shared-library and file requirements, resolve
from the root's repositories in one transaction. A test never has to name the
libraries the package links. `packages` is only for tools the test itself runs,
such as a shell or `grep`.

`--save-test-root`, `--test-root` and `--test-root-lock` need a single test
environment. If a recipe has several, select one with `--test NAME` or
`--test OUTPUT/NAME`.
The one exception is a package that deliberately replaces what the test root
already has, such as a distribution default implementation: that install is
retried without dependency checking and a warning names the replacement. A
genuinely missing requirement still fails the build.

### Ship a test root

A test root can be prepared once and moved to the machine that runs the tests:

```console
# build host: run the tests and keep the root (with the package installed)
rpmx build package.yaml --run-tests --save-test-root ./test-root

# copy ./test-root and ./test-root.lock.yaml, then on the target:
rpmx test package.yaml --test-root ./test-root --test installed
rpmx test package.yaml --test-root ./test-root --test hello/smoke
```

The saved directory carries its lock beside it (`<dir>.lock.yaml`), which is
why `--test-root` needs nothing else. On a machine of the target architecture
the tests run natively; elsewhere they run under binfmt.

---
