# Testing

## Tests

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
same output and `test.environment.contents.packages` set share a root; a
different output or set gets its own root. A recipe-level installed test sees
every output. An output-level installed test sees that output plus the sibling
outputs that provide its runtime dependencies, so a missing `Requires` between
outputs is not hidden by installing the whole output set. If a test root cannot
install its packages, add the missing runtime packages to that test's
`environment.contents.packages` (or the package-wide
`build.dependencies.tests`) or pass `--test-root-lock` on `build`/`rebuild`.

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
