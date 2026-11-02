# Testing

## Tests

### Build and test in one run

```console
rpmx test
rpmx test --test cli
rpmx test --test unit --test installed --junit report.xml
rpmx test --json
```

This builds (source, SRPM, RPMs) with tests enabled, then runs the selected
tests with the workspace and the produced RPMs available. `--json` lists the
produced RPMs; use `--junit` for per-test results.

### Test existing RPMs

```console
rpmx build
rpmx test --install-dir packages/x86_64
rpmx test --install-dir packages/x86_64 --test installed
rpmx test --install-dir packages/x86_64 --json --junit report.xml
```

- Reads `*.rpm` from the directory, ignores `*.src.rpm`.
- Skips the build entirely.
- Runs installed-stage tests only. With no `--test`, build-stage tests are
  skipped; naming one is an error.
- Needs a root lock, not an assembled `--root`; the test root is prepared from
  the lock and the supplied RPMs. For a cross package the target sysroot lock
  is used, so the test root is the target architecture.
- `--test-root-lock FILE` pins the test root instead of resolving one. With
  `--install-dir` the pair is self-contained: no build root, no profile and no
  resolution, so tests can run where the package's architecture is native.
- JSON: `{schema, ok, command: "test", tests: [{name, stage, ok, duration_ms,
  message}]}`.
- Exit 6 on failure.

Test roots are disposable and use real dependency checking. If the test root
cannot install its packages, add the missing runtime packages to
`build.dependencies.tests` or pass `--test-root-lock` on `build`/`rebuild`.

---
