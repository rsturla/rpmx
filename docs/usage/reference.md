# Reference

## Linting and errors

```console
rpmx lint
rpmx lint --strict
rpmx lint --json
rpmx lint --profile fedora-44
```

`lint` parses, plans and applies semantic rules. Current rules include:

- SPDX syntax for `license`;
- empty/very short or very long summary;
- release starting at zero;
- missing `source-date-epoch`;
- duplicate dependency or tool entries;
- duplicate source/output/test names;
- missing patch files;
- `http` locators;
- missing maintainers;
- empty tests and empty build pipelines;
- unused `vars`.

`--strict` makes warnings fatal. Every diagnostic has a stable code such as
`RPMX-LINT-010`.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. |
| 2 | Recipe, schema, plan or lint error; usage error. |
| 3 | Lock drift (recipe, sources, steps, root closure). |
| 4 | Root or source resolution/fetch failure. |
| 5 | Build or packaging failure. |
| 6 | Test failure. |
| 7 | Policy or signing failure. |
| 8 | SBOM or extraction/evidence failure. |
| 9 | Internal or I/O failure. |

`doctor --strict` uses 9 when a required host feature is missing.

---

## JSON output

Every command with `--json` writes exactly one document to stdout. Logs and
step output go to stderr, so this is safe:

```console
rpmx build --json 2>/dev/null | jq .artifacts
```

Success:

```json
{
  "schema": 1,
  "ok": true,
  "command": "build",
  "artifacts": [
    {"kind": "srpm", "path": "packages/x86_64/src/hello-0.1.0-1.fc44.src.rpm"},
    {"kind": "rpm", "path": "packages/x86_64/hello-0.1.0-1.fc44.x86_64.rpm"},
    {"kind": "evidence", "path": "packages/x86_64/hello-0.1.0-1.fc44.build.spdx.json"},
    {"kind": "repository", "path": "packages/x86_64/repodata/repomd.xml"}
  ]
}
```

Failure:

```json
{
  "schema": 1,
  "ok": false,
  "command": "build",
  "diagnostics": [
    {
      "severity": "error",
      "code": "RPMX-SANDBOX-002",
      "message": "step 'compile' failed with exit 3\n  command: ...\n  cwd: ...\n  help: ...",
      "path": null,
      "line": null,
      "column": null
    }
  ]
}
```

`lint --json` includes `summary.errors`, `summary.warnings` and all
diagnostics. `explain --json`, `test --install-dir --json`, `doctor --json` and
`steps list --json` use the same envelope.

---

## Environment variables

| Variable | Effect |
|---|---|
| `XDG_CACHE_HOME` | Base for the CAS and root cache. |
| `TMPDIR` | Location of temporary work directories (large builds need space). |
| `RPMX_PROXY` / `RPMX_ACQUISITION_PROXY` | Default acquisition proxy. |
| `RPMX_QEMU_SYSTEM` | Override the `qemu-system-<arch>` binary. |
| `<VAR>` named by `--signing-passphrase-env` / `--passphrase-env` | Signing key passphrase. |

Steps see a fixed environment: `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`,
`TZ`, `SOURCE_DATE_EPOCH`, `SRC`, `BUILD`, `STAGE`, `PREFIX`, `LIBDIR`,
`JOBS`, `RPMX_HOST_ARCH`, `RPMX_TARGET_ARCH`, `RPMX_CROSS` and
`RPMX_DEBUGINFO`. The host environment is not inherited. `SOURCE_DATE_EPOCH`,
`SRC`, `BUILD`, `STAGE`, `PREFIX`, `LIBDIR` and `JOBS` are reserved and cannot
be set by a step.

---

## Current limitations

- Typed `features` are not implemented yet.
- Typed `actions` (systemd, sysusers, tmpfiles, alternatives) are not
  implemented; use `scriptlets` raw shell.
- `update:` metadata and `rpmx update` are not implemented; `rpmx bump` handles
  manual version updates.
- `build.dependencies.tools/host/target/tests` all land in one root; there is no
  separate sysroot for cross builds.
- Cross-compilation is not implemented. Foreign targets use bubblewrap + host
  binfmt by default; `--runner qemu` selects the full-VM path and needs an
  assembled transport appliance. The build root is not bootable.
- `stage: package` tests, ABI tests and reproducibility tests are not
  implemented.
- Rich dependencies are not implemented; weak dependencies are.
- SELinux labels, `%ghost`, `%lang` and arbitrary xattrs are not implemented;
  file capabilities are.
- `-debugsource` and per-subpackage debuginfo are not implemented.
- Step-level `timeout` is parsed but not enforced individually; use
  `--step-timeout`.
- Local `file://` repository paths are host-specific and not portable.
- There is no config file yet; use flags and profiles.

---
