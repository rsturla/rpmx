# Testing

Recipes declare named cases with the same syntax in two places:

- `build.tests` checks the source workspace, such as an upstream test suite.
- Output or recipe `tests` accepts existing binary RPMs through their installed interfaces.

`rpmx build` and `rpmx rebuild` run source cases before packaging. They never
run package acceptance. `rpmx test` requires `--install-dir`; it never builds.
A release script should build, accept the resulting RPMs, then publish only
when both reports pass.

```console
rpmx build package.yaml --output packages
rpmx test package.yaml --install-dir packages/x86_64 --test-output acceptance
rpmx test package.yaml --install-dir packages/x86_64 --test hello/behavior
```

`--test NAME` selects a case; use `OUTPUT/NAME` to disambiguate output cases.
Repeat it for several cases. `rpmx plan package.yaml --json` lists all planned
source and acceptance cases. `rpmx build --no-tests` skips only source cases.

## Declare cases

Choose exactly one execution form: `script`, `argv` or `pipeline`.
Scripts and shell pipeline steps use strict Bash. Shell `-c` commands in
`argv` are errors; use `script` for shell syntax. Direct commands can assert
exact exit status and output with `expect`:

```yaml
outputs:
  - name: example
    tests:
      - name: missing-input
        argv: [example, missing-file]
        expect:
          exit-code: 1
          stdout: ''
          stderr: "missing input\n"
```

Put case-only tools in `packages`. Output acceptance selects that output and
its declared runtime siblings; recipe-level acceptance selects every output.
Do not add runtime libraries as tools to hide missing package requirements.
Bash and coreutils are baseline tools, and reusable steps add their declared
requirements. Different output/tool sets prepare separate environments.

Source cases extend the exact pinned build closure with their own tools.
They run in independent source-workspace copies after the build pipeline,
with the staged payload read-only. Test tools cannot replace packages in the
pinned build closure and do not enter the build root or another case. Tools
needed by configure or compilation remain `build.packages` dependencies.

```yaml
build:
  tests:
    - name: upstream
      packages: [dejagnu]
      timeout: 4h
      files: tests
      artifacts: [build/test-results]
      script: bash "$TEST_FILES/upstream.sh"
```

`environment` sets variables for the whole case. A pipeline step's `env`
entries take precedence. `timeout` bounds the complete case, including all
steps. An individual step may impose a shorter deadline. Durations use a
positive whole number followed by `s`, `m` or `h`; invalid values fail planning.

Cases have no external network by default. Acceptance may explicitly set
`network: true`; an offline run records such a case as `not-run` and the run
as incomplete. Source cases reject network access and transaction mode.
Use fixed local data and loopback services for deterministic package tests.

## External files and artifacts

Keep source inputs, vectors and expected results beside the recipe:

```text
package.yaml
tests/
  consumer.c
  include/check.h
  input.bin
  expected.bin
```

Set `files: tests` on a case. `TEST_FILES` points to its private read-only copy.
Contents and permission modes are captured in an immutable internal snapshot;
checkout edits cannot change a running case. Fixture contents are included
in the recipe bundle and SRPM. Executable modes affect bundle identity.
Copy an input into the workspace before modifying it.
Symlinks, missing or empty directories, parent traversal and absolute paths
are rejected. No checkout is needed after the bundle has been acquired.

Ordinary build/source-assembly steps can also declare `files`, exposed through
`RPMX_FILES`. Their fixture and artifact paths follow the same rules.
Case fixtures consistently use `TEST_FILES`, including source cases.

`artifacts` lists literal workspace files or directories to preserve on success
or failure. Paths must be relative and cannot contain globs or traversal.
Keep upstream reports as artifacts; a nonzero suite status must still fail.
Preserve the provenance and checksums beside checked-in binary vectors.

Package acceptance has no source tree. `TEST_PACKAGE_ROOT` is a read-only view
of the selected candidate RPM payload, without dependency package files.
Use it for payload layout and ELF inspection. Use installed paths to exercise
runtime behavior. `TEST_RPMS` contains the supplied binary RPMs for transaction
checks. Do not use the build's `STAGE` for package acceptance.

## Package transactions

Set `mode: transaction` when a case needs to modify installed package state.
Ordinary installed mode is the default and does not need a declaration.

```yaml
tests:
  - name: reinstall
    mode: transaction
    script: |
      rpm -q example
      rpm -e example
      if rpm -q example; then exit 1; fi
      rpm --nosignature -i "$TEST_RPMS"/example-*.rpm
      example --version
```

The selected outputs start installed. Every transaction gets a private writable
root and RPM database; changes cannot affect another case or its baseline.
rpmx adds the `rpm` tool. Verification policy remains the distribution's policy:
use explicit `--nosignature` only for a fixture intentionally generated unsigned.
Use pinned previous-version fixtures to test upgrades, configuration preservation,
`.rpmnew`, `.rpmsave`, scriptlets and removal where relevant.

Native bubblewrap cases use full subordinate UID/GID maps. Ordinary cases run
as UID/GID 65534, mapped to the caller, with no capabilities. Transactions map
the caller to namespace root. All backends grant transactions the same bounded
capabilities: `CHOWN`, `DAC_OVERRIDE`, `FOWNER`, `FSETID`, `SETGID`, `SETUID`
and `SYS_CHROOT`. These permit RPM ownership changes and account scriptlets in
the private root. Both modes set no-new-privileges and prevent nested user namespaces.
Missing prerequisites fail setup; there is no single-ID fallback. See
[host requirements](getting-started.md#requirements) and `rpmx doctor --strict`.

## Runtime library linkage

A shared-library output should check its own runtime dependency closure without
installing a compiler or Python runtime that could supply missing libraries:

```yaml
outputs:
  - name: example-libs
    tests:
      - name: runtime-linkage
        pipeline:
          - uses: test/runtime-linkage
            with:
              libraries: [libexample.so.1]
```

The installed ELF loader resolves each requested library and its dependencies.
Keep real API, error-path and ownership tests on development outputs as well;
loader success alone does not establish library behavior. `test/hardening-check`
inspects candidate payload ELF protections through `TEST_PACKAGE_ROOT`.

## Repository policy and replay

Use a test profile to choose acceptance repositories independently of the build:

```console
rpmx test package.yaml --install-dir packages/x86_64 \
  --test-profile profiles/runtime.yaml
```

Its target architecture, prefix and library directory must match the package.
Its release version can differ. Relative repository paths resolve against the
profile file. The profile may omit `packages`; its install/reinstall seed lists
are ignored. Each root contains only baseline tools, explicit case tools and
the selected outputs' actual runtime closure. Exact candidate RPMs win over
identical published NEVRAs without disabling normal provenance checks.

Save complete recipe-bound environments for a later replay:

```console
rpmx test package.yaml --install-dir packages/x86_64 \
  --test-profile profiles/runtime.yaml --save-environment saved-environments
rpmx test package.yaml --install-dir packages/x86_64 \
  --replay saved-environments --offline
```

`--replay` accepts saved roots and their private binding, not arbitrary bare
locks or root directories. It validates the recipe, subjects and execution
policy. Do not combine it with `--test-profile`. Offline acceptance requires a
replay; fresh resolution and assembly can acquire metadata, RPMs or images.
Target runtime behavior needs a native target machine or a supported emulation
runner. A successful cross compilation alone does not establish runtime coverage.

## Reports and diagnostics

Each application invocation writes `run-report.json` and `run-report.xml` under
its output directory before temporary resources are removed. Standalone test
output defaults to `test-results/`; use `--test-output DIR` to choose a location.
`--junit FILE` writes another JUnit copy. `--json` emits the raw report on stdout;
progress and command output use stderr.

The report document is `rpmx-run-report`, version 1. It records input artifacts,
resolved environments, output artifacts, case evidence and diagnostics.
`build_tests_requested` and `build_tests` describe source checks; `acceptance`
is `not-requested`, `running` or `completed`. Each acceptance case appears in
`tests`, with status `passed`, `failed`, `skipped` or `not-run`, plus its reason
and primary diagnostic. Skipped or not-run cases make the run incomplete;
they do not establish coverage.

Require `outcome: passed`, completed acceptance and every selected case passed
before release. An incomplete or failed run is not success. Source and acceptance
reports remain separate so a passing build cannot imply unrequested acceptance.
Logs and declared artifacts survive setup failures and workspace cleanup where
the relevant evidence has been created.

Strict script failures report the command, source line and status. For pipelines,
all component exit statuses remain visible. Handled failures inside `if`, `!`
or `||` retain normal Bash behavior. Include useful assertion messages in external
fixtures, and keep outputs locale-independent. For a source failure, retain its
workspace with `--keep-workdir` and use `rpmx shell` to inspect the pinned build
environment. A debug shell uses the same sandbox policy, not the host root.
