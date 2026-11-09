# Troubleshooting

## Troubleshooting

**`RPMX-ROOT-016: a full build requires --root-lock and --root`**  
The recipe has no `build.root` and no `--profile`. Add a root or pass explicit
root options.

**`RPMX-ROOT-016: root ... is not cached ...` with `--offline`**  
Resolve once without `--offline`, or use the root lock that produced the cache.

**Root resolution is slow**  
The first resolve downloads and assembles the closure. Later builds reuse
`~/.cache/rpmx/roots/`. `rpmx doctor` checks dnf5 and the runner.

**Test root install fails**  
Strong runtime requirements of the selected RPMs are resolved into the test
root automatically. A missing package here normally means the requirement is
not available in the configured repositories or the test pins
`--test-root-lock` to an older closure. Add it to that test's
`environment.contents.packages`, refresh the pinned closure, or check the
repository configuration.
Ensure `TMPDIR` has space; a test root is a full distribution root.

**An output-level test cannot find a sibling output**

The test root installs only the output under test and the sibling outputs that
satisfy its RPM dependencies. Add the missing relationship to the output's
`dependencies.run`, or use a recipe-level test when the test really needs every
output.

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

**`RPMX-BOOTSTRAP-009: no such bootstrap stage`**

The name is not in this manifest and architecture. Run
`rpmx bootstrap plan MANIFEST` and copy a stage name from the output.

**`RPMX-BOOTSTRAP-009: this manifest declares several architectures`**

Pass `--arch` to `rpmx bootstrap plan` or `rpmx bootstrap build`. Each
architecture is an independent ladder.

**`RPMX-BOOTSTRAP-007: stage input ... is missing`**

Build the input stage first. The message prints the required command, for
example `rpmx bootstrap build bootstrap.yaml --stage stage0 --arch x86_64`.

**`RPMX-BOOTSTRAP-008: stage output ... is not empty`**

The stage repository already exists. Use `--force` only when replacing it is
intentional; the build writes a temporary sibling and publishes atomically.

**`RPMX-BOOTSTRAP-006` with `--locked`**

The stage profile or extras changed, or the lock/root pair is incomplete. Run
the same command without `--locked` once to resolve and write fresh locks, then
commit them and rerun with `--locked`.

**`RPMX-BOOTSTRAP-004: cycle`**

Add an explicit `order` edge that breaks the cycle, or list every member of the
cycle in the stage's `fixed-point`. Fixed point is the only place a package
cycle is meaningful.

**`RPMX-BOOTSTRAP-005: fixed point did not converge`**

Inspect the package names and root digests in the message. A non-reproducible
compiler, wall-clock timestamp or unordered archive makes the bytes change
every pass. Fix the recipe or raise `max-iterations` only when convergence is
genuinely slow, not when the output is unstable.

**A stage root is not cached with `--offline`**

Resolve once without `--offline`, or run the online command named in the error.
Bootstrap does not fetch repositories during the build.

**`rpmx rebuild` rejects a stage lock**

That is intentional. Plain `rpmx rebuild` requires the embedded lock to match.
Use `rpmx bootstrap rebuild package.src.rpm --root-lock ... --root ... --output
... --srpm-out ...` to replace the lock and emit a new SRPM. The input SRPM stays
byte-identical, and `--srpm-out` supplies the new SRPM directory.

---
