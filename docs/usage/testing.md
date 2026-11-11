# Testing

## Tests

### Package transactions

Use `stage: transaction` for tests that change the installed package set:

```yaml
tests:
  - name: reinstall
    stage: transaction
    script: |
      rpm -q example
      rpm -e example
      if rpm -q example; then exit 1; fi
      rpm -i "$TEST_RPMS"/example-*.rpm
      example --version
```

rpmx prepares the package root as for installed tests. The built outputs start installed.
Each transaction test gets a private writable copy, with an independent RPM database.
Bubblewrap runs transaction commands as root inside its user namespace.
The prepared root and other tests retain their original files and package state.
rpmx adds the `rpm` tool to the test dependencies.

`TEST_RPMS` points to copies of the supplied build outputs inside the test workspace.
Installed tests can also inspect these RPM files when they are available.
Use pinned previous-version RPMs in `files` to establish an upgrade baseline.
Then change a configuration file, install the new RPM and assert the preserved value.
Check `.rpmnew`, `.rpmsave` and scriptlet effects where the package requires them.
Declare any additional tools used by these checks in `packages`.

For a saved root, supply the RPM directory with the root:

```console
rpmx test package.yaml --test-root ./test-root --install-dir ./rpms --test reinstall
```

The saved root supplies the baseline. This command does not install the supplied RPMs before the test.
Transaction tests reject the host root `/` and require supplied RPM files.
`--skip-installed` skips both installed and transaction tests.
Root copies use host `unshare` and GNU `cp`, with reflinks where the filesystem supports them.

The lifecycle regression test builds two RPM versions and checks install, upgrade, configuration preservation and erase:

```console
RPMX_TRANSACTION_TEST_ROOT=/path/to/root \
RPMX_TRANSACTION_TEST_LOCK=/path/to/root.lock.yaml \
cargo test -p rpmx --test transactions -- --ignored
```

Use a native prepared root that contains Bash, coreutils and RPM.

### Results and failure artifacts

`rpmx build` keeps test logs under its output directory in `test-results/`.
For existing RPMs or saved roots, use `rpmx test --test-output DIR`.
The default directory for these standalone runs is `./test-results`.
Each test has separate `stdout.log` and `stderr.log` files. Output still appears while commands run.

Before preparing each installed-test environment, `rpmx test` prints its group
number, output and explicit tools to stderr. JSON results stay on stdout.
The test output directory also keeps `environments/N/environment.json` and
`environments/N/root.lock.yaml`. The JSON identifies the group's tests and
records `preparing`, `ready` or `error`. The lock records the exact resolved
dependency closure without repository authentication settings. The supplied
RPMs under test remain separate from this dependency lock.

These files survive temporary root cleanup. An assembly failure retains the
resolved lock when resolution completed. A resolution failure records its
diagnostic code without a lock. Each new invocation clears old environment
metadata so a failed preparation cannot reuse evidence from an earlier run.

Declare workspace files or directories to preserve, including upstream reports:

```yaml
tests:
  - name: upstream
    stage: build
    artifacts: [build/meson-logs]
    pipeline:
      - uses: meson/test
```

rpmx collects artifacts after success or failure. Missing paths are ignored.
Paths must be relative to the test workspace. Symlinks and parent traversal are errors.
Nested upstream reports remain intact in the artifact directory.
The JUnit report includes the last 64 KiB of each output stream.
Full output remains in the log files.

JSON test results use `status` values: `passed`, `failed`, `error` and `skipped`.
Command failures and timeouts are test failures. Runner and environment failures are errors.
JUnit uses separate failure, error and skipped elements. Setup failures also produce a JUnit error.
Add `skip: reason` to record an explicit skip. A run that selects only skipped tests needs no root.
Remove the skip when the test can run. A skip does not establish coverage.

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

### Runtime library linkage

Check that an installed library output has its required shared-library dependencies:

```yaml
outputs:
  - name: example-libs
    tests:
      - name: runtime-linkage
        stage: installed
        pipeline:
          - uses: test/runtime-linkage
            with:
              libraries: [libexample.so.*, libexample-extra.so.*]
```

`libraries` is a required, nonempty list of SONAME basename patterns.
The step searches the profile's `libdir` and requires at least one ELF file for every pattern.
It skips non-ELF files such as Python GDB helpers, even when their names match.
The installed ELF loader checks each library's dependency graph.
A missing loader, missing library pattern or missing dependency fails the test and names the path.
The step uses only the baseline Bash and coreutils tools.
Keep it on the runtime output with no compiler or Python test packages, so extra tools cannot supply missing runtime dependencies.
Pair it with compile, link and execution tests on development and static outputs to exercise the exported APIs.

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
- JSON: `{schema, ok, command: "test", tests: [{name, stage, status, duration_ms,
  message, skip_reason, results}]}`. An output-level test uses its qualified name, `output/test`.
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

The supplied RPM wins when a repository already publishes the same name,
epoch, version, release and architecture. rpmx excludes that exact package
from the other test-root repositories. Other versions remain available for
dependency resolution, and normal repository provenance checks still apply.

The resolver regression builds a package, publishes an identical copy in a
local repository, and runs its installed test through a fresh test root:

```console
RPMX_TEST_RESOLUTION_LOCK=/path/to/native-root.lock.yaml \
  cargo test -p rpmx --test published_tests -- --ignored
```

The root lock needs a builder image and reachable repositories for Bash,
coreutils and their runtime dependencies. This test uses Podman and can
download repository metadata and packages.

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
