# Troubleshooting

## Troubleshooting

**`RPMX-ROOT-016: a full build requires --root-lock and --root`**  
The recipe has no `build.root` and no `--profile`. Add a root or pass explicit
root flags.

**`RPMX-ROOT-016: root ... is not cached ...` with `--offline`**  
Resolve once without `--offline`, or use the root lock that produced the cache.

**Root resolution is slow**  
The first resolve downloads and assembles the closure. Later builds reuse
`~/.cache/rpmx/roots/`. `rpmx doctor` checks dnf5 and the runner.

**Test root install fails**  
The test root needs runtime dependencies. Add them to
`build.dependencies.tests`, or use `--test-root-lock` with a pinned closure.
Ensure `TMPDIR` has space; a test root is a full distribution root.

**A step failed**  
Read the `command`, `cwd` and help line. Re-run with `--keep-workdir` and enter
the workspace with `rpmx shell` (using the root and paths printed by the build),
or add `set -eu`/`set -x` to the step. `--debug` sets `set -x`.

**`rpmx root digest` fails with permission denied**  
Some roots contain mode-000 files. The command falls back to `unshare -Ur` or
`podman unshare`; install one of them, or run as a user namespace that can read
the tree.

**A local `file://` repository is ignored**  
Use an absolute path that exists on the resolve host. It is mounted read-only
during `root resolve`; `root assemble` uses the CAS, not the repository.

**`RPMX-ROOT-019` during root resolution**  
The repository needs credentials, a credential reference could not be read, or
the server rejected them. Add an `auth` block to the profile, export the
referenced environment variables, and make sure the profile is passed with
`--profile FILE` or lives under `$XDG_CONFIG_HOME/rpmx/profiles`. A profile
named by the recipe cannot carry credentials. If the repository uses signed
metadata, set `repo-gpgcheck: true` and supply its `gpgkey`.

**`--locked` reports drift**  
Review `rpmx lock --check` output, run `rpmx lock`, and review the
`package.lock.json` diff. Do not hand-edit step digests.

---
