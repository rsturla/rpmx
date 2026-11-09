# Bootstrap ladder implementation plan

Status: proposed

Issues covered:

- #1 Native bootstrap builds: separate build root from target sysroot
- #2 Add a bootstrap/stage plan for ordered multi-stage toolchain rebuilds
- #3 Allow stage-specific build dependencies for bootstrap
- #4 Add a bootstrap rebuild mode with a replacement root lock

This document is an implementation plan, not a final design record. It maps each
issue to the existing code paths and proposes a delivery order that keeps every
intermediate state shippable.

## 1. Where the code is today

The relevant machinery already exists:

- **Two roots per build.** `Profile::build_root_profile` and
  `Profile::sysroot_profile` (`crates/rpmx-root/src/profile.rs`) derive a host
  build root and a target sysroot from a `cross` block.
  `autoroot::requirement` (`crates/rpmx-cli/src/commands/autoroot.rs`) merges
  the plan's `tools`/`host`/`target` needs into the right half, resolves both
  through the CAS, and returns an `AutomaticRoot` containing both roots.
- **Cross execution contract.** `SandboxConfig` mounts the sysroot read-only at
  `/sysroot`, exports `RPMX_CROSS`, the triple, `CC`/`CXX`/`AR`/`LD`/`STRIP`/
  `OBJCOPY` and sysroot-aware pkg-config/CMake/Meson files
  (`crates/rpmx-runner/src/sandbox/mod.rs`). Steps run on the host.
- **Cross checks.** `rpmx-build/src/elf.rs` checks the ELF machine and every
  `DT_NEEDED` against the sysroot; `rebuild/mod.rs` runs the link-closure check
  for any assembled root.
- **Cross provenance.** A cross SRPM embeds `root.lock.json`,
  `sysroot.lock.json` and a `cross` block (`rpmx-store/src/srpm.rs`,
  `rpmx-build/src/source_package.rs`), so `rpmx rebuild` needs no profile.
- **Strict rebuild.** `rebuild/prepare.rs` refuses a `--root-lock` that differs
  from the embedded lock with `ROOT_016`.
- **Root locks and repositories.** `rpmx root resolve/assemble`, `RootLock`,
  `Repository`, `repo::index`, and local `file://` repositories already provide
  the pieces a stage ladder needs.

The gaps are therefore mostly policy and orchestration, not new build engines.

## 2. Shared design decisions

### 2.1 Profile spelling for native bootstrap (#1)

Reuse the existing `cross` shape and add one explicit marker:

```yaml
document: rpmx-profile
version: 1
name: eternal-bootstrap-stage1
archs: [x86_64]                       # target
repositories: [...]
packages:
  install: [glibc, glibc-devel]       # target sysroot
cross:
  host: x86_64                        # same CPU as target
  bootstrap: true                     # allow host == target
  triple: x86_64-linux-gnu            # optional; see below
  packages: [bash, coreutils, gcc, make, ...]  # seed build root
```

Semantics:

- `cross.bootstrap: true` means "the build root and the target sysroot are
  separate even when the architecture is the same". It changes validation only;
  it does not introduce a second execution strategy.
- Without `cross.bootstrap`, the current `ROOT_006` rejection of
  `cross.host == archs` remains unchanged.
- `cross.bootstrap` works for foreign targets too, but it is a no-op there
  because the two roots already differ.
- Derived profiles remain ordinary native profiles with `cross = None`, so
  cache identity, assembly and lock handling need no special case.

Toolchain command names:

- If `cross.triple` is set, keep the GNU default (`<triple>-gcc`,
  `<triple>-g++`, ...). This is the **recommended ladder path**: the stage 1
  cross toolchain needs an explicit triple so GCC finds the target CRT
  objects, `libgcc`, and the sysroot libraries.
- If `cross.bootstrap: true` and `cross.triple` is omitted, use native command
  names (`gcc`, `g++`, `ar`, `ld`, `strip`, `objcopy`) while keeping the target
  triple for `RPMX_TARGET_TRIPLE`. This is the **fallback** for a seed compiler
  whose native command names already target the same architecture; it makes a
  seed compiler plus `--sysroot=/sysroot` usable without wrapper packages.
- Record which form was used in the SRPM. Add a `native: bool` (or an explicit
  command map) to `LockCross` so `rpmx rebuild` reconstructs the same toolchain.
  Without this, a native bootstrap SRPM would rebuild with
  `x86_64-linux-gnu-gcc` even though the build used `gcc`.

### 2.2 Bootstrap manifest (#2 and #3)

Use a dedicated document. Accept `schema: 1` as an alias for `version`, because
the issue examples use it, but document `document`/`version` as the canonical
form:

```yaml
document: rpmx-bootstrap
version: 1
name: eternal
archs: [x86_64]                    # optional allowed set
recipe-root: recipes               # default "."
roots: bootstrap/roots             # default, relative to this file
locks: bootstrap/locks             # default, relative to this file
source-date-epoch: 1704067200      # deterministic stage repo metadata

seed:                              # optional but recommended for a ladder
  name: fedora-44
  image: registry.fedoraproject.org/fedora:44
  releasever: "44"
  generation: "2026.01"

stages:
  - name: stage0
    profile: profiles/stage0.profile.yaml
    packages: [filesystem, binutils, glibc, gcc, make, bash, coreutils]
    output: out/stage0
    seed: true                     # seed stage: explicit package list required
    extra-tools: [bison, python3, texinfo]
    extra-sysroot: []              # target-only extras for a cross stage

  - name: stage1
    profile: profiles/stage1.profile.yaml
    packages: [binutils, glibc, gcc, make, bash, coreutils]
    output: out/stage1
    inputs: [stage0]               # default: the immediately previous stage

  - name: stage2
    profile: profiles/selfhosted.profile.yaml
    packages: all                  # allowed only for the final self-hosted stage
    output: out
    fixed-point: [binutils, glibc, gcc]
```

Stage fields:

Paths in the manifest resolve relative to the manifest file. Parent components
are allowed, so a manifest in `.bootstrap/` can reference `../packages` and
`../profiles/...`; absolute paths remain supported but are not required.

- `profile`: required; a normal profile file, resolved relative to the
  manifest.
- `packages`: package names, `all`, or both. Recipes are discovered by scanning
  `recipe-root` for `package.yaml` files and reading `package.name`. `all`
  selects every discovered recipe. Duplicate package names are an error.
- `recipes`: optional explicit recipe paths/globs for callers that do not want
  discovery. `packages` filters the selected set when both are present.
- `output`: required; a directory. The build writes `<output>/<arch>` exactly as
  `rpmx build` does, and bootstrap writes repository metadata there.
- `inputs`: optional stage names; default `[previous stage]`. Stage 0 defaults
  to no bootstrap inputs. Forward references and cycles are rejected.
- `seed`: marks a seed/bootstrap stage. In v1 this is not just provenance: a
  seed stage (and any stage whose profile uses `cross`) must name an explicit
  `packages` list or an explicit `recipes` list and may not use `packages: all`,
  so a new recipe cannot accidentally enter the bootstrap closure. `all` is
  allowed only for the final self-hosted stage. `seed` is also the future
  selection point for recipe-level bootstrap dependencies.
- `extra-tools`: build-root packages added to this stage only (#3).
- `extra-sysroot`: target packages added to this stage's sysroot only.
- `order`: optional explicit edges, for example
  `order: [[glibc, gcc], [gcc, glibc]]`.
- `replaces`: optional map from an Eternal output package to the seed package
  name or names it replaces. It lets a stage hide a differently named seed
  provider; fixed-point passes hide those names in every other repository.
- `fixed-point`: optional package names rebuilt until output and root digests
  stabilise.
- `source-date-epoch`: optional per-stage override for `repo::index`; defaults
  to the manifest value.

Manifest-level fields:

- `source-date-epoch`: optional; the deterministic timestamp for
  `repo::index`. A stage may override it. If both are absent, bootstrap derives
  the value from the stage recipes and fails if they disagree. This must be in
  the schema now so a yearly rebootstrap can produce byte-stable stage
  repository metadata.
- `seed`: optional metadata block with `name`, `image`, `releasever` and
  `generation`. It is printed in the plan and build log and copied into the
  optional `seed`/`generation` fields of each stage `RootLock` (outside
  `closure_digest`), so locks are self-describing. The build-level SBOM can
  carry the same labels as annotations in a follow-up. It is metadata, not a
  new resolution mechanism; the root lock still records builder image and
  repositories.
- `seed` is intentionally overloaded by scope: the top-level `seed:` is
  provenance metadata, while `seed: true` inside a stage marks the seed stage.
  The plan output must print both explicitly so the distinction is obvious. If
  `seed.image` or `seed.releasever` is present and disagrees with the stage 0
  profile, the plan fails early (`BOOTSTRAP_001`) instead of silently recording
  two different seeds; the profile and its root lock remain the source of truth.

Why stage-level `extra-tools` for #3 instead of a recipe-level
`build.bootstrap` list:

- It satisfies the acceptance criteria: the tool is in the stage root lock and
  build log, is absent from later stages, and cannot leak into runtime
  dependencies.
- It keeps `package.yaml` stage-agnostic, so the same recipe really is reused
  across stages.
- It avoids making planning, lock verification and SRPM rebuild aware of a
  second conditional dependency list.

A recipe-level `build.bootstrap` list remains a reasonable future
extension if a distro needs per-recipe seed-only tools. The manifest stage can
then select it with `seed: true`. Do not add both mechanisms in v1.

### 2.3 Explicit root pair

Bootstrap must build every recipe in a stage against the same pre-resolved root
pair, not resolve a slightly different root per recipe. Refactor the existing
flow so this is a first-class internal input:

- Add `RootLocation` to `autoroot`: either `Cache` (today's behaviour) or
  `Explicit { lock, root }` for stage roots.
- Extract the core of `requirement()` into
  `requirement_for_profile(profile, required_packages, name, arch)`, and make
  `RequiredPackages` and its merge helper public to the crate.
- Extract the post-root portion of `build::build_one` into an internal
  `execute_with_roots(...)` that accepts:
  `root_lock`, `root`, `sysroot`, `sysroot_lock`, `sysroot_packages`,
  `CrossToolchain`, and `exec`.
- Add an optional in-memory `Profile` override to `requirement`,
  `lock::build` and `lock::verify`. Bootstrap can then use the generated stage
  profile without writing credentials or host-specific paths into a reviewable
  file.
- Optionally expose the explicit pair on the CLI as `--sysroot-lock` and
  `--sysroot` for `build` and `rebuild`. This is useful for debugging a stage
  manually and is required for #4's native-cross replacement case. It also
  gives `build --profile P --root-lock L --root R --sysroot-lock S --sysroot X`
  a coherent meaning: the profile supplies the toolchain and triples, the flags
  supply the already-assembled roots.

For bootstrap stages, `--locked` must pin the *stage* root, not a per-recipe
root requirement. Add a `RootPolicy` to `lock::verify`:

- `Require`: today's behaviour.
- `Stage`: verify recipe digest, sources, steps, cells and policy, but take the
  root/sysroot pins from the active stage lock. This is what makes one recipe
  lock reusable in stage 0 and stage 2 even when `extra-tools` differ.

### 2.4 Error codes and exit codes

Add a `BOOTSTRAP` diagnostic domain:

| Code | Meaning | Exit |
|---|---|---|
| `RPMX-BOOTSTRAP-001` | Manifest malformed, unsupported version, or unknown field | 2 |
| `RPMX-BOOTSTRAP-002` | Invalid stage name/path/output/input reference | 2 |
| `RPMX-BOOTSTRAP-003` | Recipe discovery/selection failed, duplicate package, multi-cell recipe | 2 |
| `RPMX-BOOTSTRAP-004` | Dependency cycle without an `order` edge or `fixed-point` exemption | 2 |
| `RPMX-BOOTSTRAP-005` | Fixed point did not converge | 5 |
| `RPMX-BOOTSTRAP-006` | `--locked` stage lock is missing or has drifted | 3 |
| `RPMX-BOOTSTRAP-007` | A stage prerequisite (input output/repo/lock) is missing | 2 |
| `RPMX-BOOTSTRAP-008` | Stage output/root exists and `--force` was not given | 2 |
| `RPMX-BOOTSTRAP-009` | Architecture or stage selection is invalid or ambiguous | 2 |

Update `crates/rpmx-cli/src/exit.rs` with explicit overrides for 005 and 006 and
a default `BOOTSTRAP -> RECIPE` mapping. Extend the exit-code tests for 007-009
and the actionable messages they carry.

## 3. Phase 1: native bootstrap (#1)

This is the prerequisite for a Wolfi-style ladder and should land first.

### 3.1 Profile and toolchain

Files:

- `crates/rpmx-root/src/profile.rs`
  - Add `bootstrap: bool` to `Cross` with `#[serde(default)]`.
  - Skip the `host == target` rejection in `validate_cross` when
    `cross.bootstrap` is true.
  - Skip the same rejection in `build_root_profile` when
    `cross.bootstrap` is true.
  - Keep the existing error text for the non-bootstrap case.
- `crates/rpmx-base/src/guest.rs`
  - Add a `CrossToolchain::native(triple)` constructor and a `native: bool`
    field.
- `crates/rpmx-cli/src/commands/autoroot.rs`
  - Choose `CrossToolchain::new(triple)` when the triple is explicit, and
    `CrossToolchain::native(triple)` when `bootstrap` is set without a triple.
- `crates/rpmx-store/src/srpm.rs`
  - Add `#[serde(default)] native: bool` to `LockCross`.
- `crates/rpmx-build/src/source_package.rs`
  - Copy `CrossToolchain::native` into `LockCross`.
- `crates/rpmx-cli/src/commands/rebuild/prepare.rs`
  - Reconstruct a native toolchain when `LockCross.native` is set.

### 3.2 Root assembly and cache correctness

Native bootstrap exercises the existing "two roots" path, but it also exposes a
latent cache issue: `autoroot::ensure` creates `<build-root>/sysroot` after the
root digest is written. That changes the build-root tree and makes the next
cache hit re-hash and potentially reject the root.

The bwrap backend cannot create the bind destination because the assembled
root is already mounted read-only, so the mount point must exist in the root.
Fix the ordering instead: make `ensure_root`/`ensure_with` create the
`<build-root>/sysroot` directory *before* `digest_root` is computed, whenever
the root pair includes a sysroot. The digest then covers the mount point
consistently, and a cache hit no longer sees a newer root than its lock.

Add a regression test that publishes a cross root, creates the mount point
before the digest, and loads it with `load_cached`. A second test should fail
if the mount point is created after publication, to keep the ordering honest.

### 3.3 Checks and provenance

No new link logic is needed:

- `unresolved_needed` already fails when a host/seed library is absent from the
  sysroot.
- The ELF machine check compares against the target, which is correct for the
  same-architecture case.
- `source_package` already embeds both locks, and `rebuild` already assembles
  the sysroot from the embedded lock.

Verify and document the expected native-bootstrap environment:

- `RPMX_CROSS=1`, `RPMX_EMULATED=0`.
- `RPMX_SYSROOT=/sysroot`.
- `RPMX_HOST_TRIPLE` and `RPMX_TARGET_TRIPLE` are both the target triple.
- `CFLAGS`/`LDFLAGS` carry `--sysroot=/sysroot`; `LD`/`AR`/`STRIP` are native
  when the native-command form is selected.

### 3.4 Tests for #1

- `profile.rs`:
  - A same-arch `cross` block without `bootstrap` still fails with `ROOT_006`.
  - The same block with `bootstrap: true` parses and derives distinct
    build/sysroot profiles.
  - `bootstrap` is preserved through `for_target`.
- `autoroot.rs`:
  - A same-arch bootstrap requirement has two distinct digests.
  - Build-root packages are not in the sysroot and vice versa.
  - `cross.native` is set when no triple is given.
- `srpm`/`rebuild` unit tests:
  - `LockCross.native` round-trips and reconstructs the native toolchain.
  - A bootstrap SRPM embeds both locks.
- An ignored/integration smoke under `examples/bootstrap/` or
  `scripts/bootstrap-smoke.sh` builds a tiny C package against a same-arch
  sysroot using a seed compiler. It is gated on `RPMX_BOOTSTRAP_SMOKE=1` because
  CI has no dnf/podman/network setup.

## 4. Phase 2: bootstrap plan and graph (#2, first step)

Land `rpmx bootstrap plan` before any stage build. It is pure, testable without
network, and gives the manifest format a reviewable surface.

Files:

- `crates/rpmx-language/src/bootstrap.rs` (new)
  - `BootstrapManifest`, `Stage`, parsing with `yaml::parse_strict`, validation.
  - Stage name/path validation, unique outputs, forward-reference checks.
  - Recipe discovery contract (scan `recipe-root` for `package.yaml`).
  - Build-order computation from `build.packages`/`build.sysroot`/test package
    names that match other stage packages, plus explicit `order` edges.
  - Cycle detection; a cycle is allowed only when every member is in
    `fixed-point`, otherwise `BOOTSTRAP_004` with the cycle names and a hint.
  - `packages: all` expansion and duplicate-name detection.
  - Seed/cross stage policy: `seed: true` or a cross profile requires an
    explicit `packages`/`recipes` list and rejects `packages: all`
    (`BOOTSTRAP_002`/`BOOTSTRAP_003`).
  - `source-date-epoch` validation and precedence (stage, then manifest, then
    the recipes; disagreeing recipe epochs are an error).
  - Optional `seed` metadata validation (`name`, `image`, `releasever`,
    `generation`).
  - Reject multi-cell recipes in v1 (`BOOTSTRAP_003`) so package names are
    unambiguous.
- `crates/rpmx-language/src/lib.rs`
  - Re-export the new module.
- `crates/rpmx-cli/src/cli.rs`
  - Add `Command::Bootstrap` with `BootstrapCommand::{Plan, Build, Rebuild}`.
  - Add `BootstrapPlanArgs`: manifest path, `--arch`, `--json`, `--verbose`.
- `crates/rpmx-cli/src/commands/bootstrap/{mod.rs,plan.rs}` (new)
  - Load the manifest, load each recipe bundle and plan per arch, compute the
    stage graph, render it, and emit a JSON document with
    `report::success("bootstrap-plan", ...)`.
- `crates/rpmx-cli/src/commands/dispatch.rs`, `commands/mod.rs`,
  `src/cli.rs::command_name`, `src/render.rs`
  - Wire the command and add a human renderer.
- `crates/rpmx-base/src/error.rs`
  - Add the `BOOTSTRAP_*` codes.

Plan output (human):

```text
stage0  profile=profiles/stage0.profile.yaml  arch=x86_64  inputs=-
  root   bootstrap/roots/stage0/x86_64
  lock   bootstrap/locks/stage0/x86_64.lock.yaml
  repo   out/stage0/x86_64        id bootstrap-stage0
  tools  +bison +python3 +texinfo
  order  filesystem, binutils, glibc, gcc, make, bash, coreutils
stage1  profile=profiles/stage1.profile.yaml  arch=x86_64  inputs=stage0
  ...
```

Tests:

- Manifest parse/validation matrix, including unknown fields and `schema: 1`.
- Topological order, cycle error, fixed-point exemption.
- `packages: all` discovery, duplicate names, missing recipes.
- Seed-stage policy rejects `packages: all`; the final self-hosted stage may
  use it.
- `source-date-epoch` precedence and disagreement diagnostics.
- `rpmx bootstrap plan --json` includes seed/generation metadata and the
  effective timestamp.
- `rpmx bootstrap plan --json` contract test and docs/help walk.

## 5. Phase 3: stage build and stage-specific deps (#2 build + #3)

This is the first phase that resolves roots and builds.

### 5.1 Stage profile synthesis

For each stage and target architecture:

1. Load the stage profile.
2. Add one repository per input stage:
   `id = bootstrap-<stage>-<input>`, `baseurl = file://<abs input output>/<arch>`.
   Use a stable repo id and reject collisions with profile repos.
3. Merge `extra-tools` into the build-root package set and `extra-sysroot` into
   the target package set.
4. Union every selected recipe's `RequiredPackages` from
   `autoroot::required_packages`; for cross stages, build needs go to
   `cross.packages`, target needs to `packages.install`.
5. Set the profile name to `<manifest name>-<stage>`.
6. Validate and derive the build-root/sysroot profiles.

The effective profile stays in memory. If `--debug`/`--keep-workdir` is set,
write it under the work directory, never under `locks/`, so repository
credential references are not committed by accident.

### 5.2 Stage root resolution

- Resolve and assemble the build root with `Assembly::Install` into
  `roots/<stage>/<arch>/root`.
- Resolve and assemble the sysroot with `Assembly::Extract` into
  `roots/<stage>/<arch>-sysroot/root` when the profile crosses.
- Write `locks/<stage>/<arch>.lock.yaml` and
  `locks/<stage>/<arch>.sysroot.lock.yaml`.
- Add optional `seed: Option<String>` and `generation: Option<String>` (or a
  generic labels map) to `RootLock`, outside `closure_digest`, populated from
  the manifest's `seed` block. This keeps locks self-describing without
  changing resolution.
- Add an optional `requirement: Option<String>` to `RootLock` (outside
  `closure_digest`) so a later run can decide whether an existing stage root is
  still valid without re-resolving. Existing locks without the field remain
  valid.
- `--locked` uses the existing stage lock and root, verifies the root digest,
  and fails with `BOOTSTRAP_006` if either is missing or changed.
- `--refresh` forces re-resolution.
- `--offline` never resolves or fetches; it fails with the existing `ROOT_016`
  message naming the stage.

### 5.3 Building a stage

For each recipe in graph order, call the refactored internal
`build::execute_with_roots` with:

- the in-memory stage profile (or a profile path for `--profile` compatibility),
- the stage build root and sysroot,
- the stage lock and sysroot lock,
- `no_index = true` (bootstrap indexes once per stage),
- `RootPolicy::Stage` for `--locked`,
- `seed_stage = true` so the build log records the stage and extras.

Build into a sibling temporary output directory. After the build:

1. Run `repo::index` on the temporary `<output>/<arch>` with the stage/manifest
   `source-date-epoch`, never wall-clock time.
2. Record the package names written to the stage output. A small helper in
   `rpmx-rpm` (for example `package_names(path)`) reads them from the RPM
   headers.
3. Write the stage lock.
4. Rename the temporary output into place.
5. Print a stage summary: root digest, package count, extra tools, output repo.

A non-empty destination is refused unless `--force` is given, so a failed
stage never leaves a partially valid repository behind.

If a recipe fails, stop the stage with the underlying diagnostic and name the
stage and recipe in the message, followed by the exact retry command.

### 5.4 #3: stage-specific dependencies

`extra-tools`/`extra-sysroot` are part of the stage requirement, so they are:

- resolved into the stage root/sysroot,
- recorded in `locks/<stage>/<arch>.lock.yaml` and the embedded SRPM root
  lock,
- printed in the stage build log,
- absent from later stages unless that stage also declares them,
- never part of the output's runtime dependency set.

No recipe model, `PlannedCell`, `LockBuild` or runtime dependency change is
needed for this version. A future recipe-level
`build.bootstrap` can feed the same stage requirement when a stage
is marked `seed: true`.

### 5.5 CLI

```console
rpmx bootstrap plan bootstrap.yaml [--arch ARCH] [--json]
rpmx bootstrap build bootstrap.yaml [--arch ARCH] [--stage NAME] \
  [--locked] [--offline] [--refresh] [--force] [--jobs N] \
  [--runner auto|bwrap|podman|qemu]
```

- With no `--stage`, build every stage in order.
- `--stage NAME` builds only that stage and requires its input outputs,
  indexes and locks to exist (`BOOTSTRAP_007`).
- Multiple architectures are independent ladders; `build` requires one `--arch`
  when the profile/manifest declares several. `plan` may expand all of them.
- `--force` allows replacing a non-empty stage output or root. Without it, the
  command refuses to overwrite existing state.
- `--refresh` re-resolves the stage root and cannot be combined with `--locked`
  or `--offline`.
- Reuse the existing `RuntimeArgs`, `CacheArgs`, runner, proxy, offline and
  signing arguments so the CLI stays consistent with `build`.

### 5.6 UX contract

Bootstrap is a long-running, multi-stage operation. The CLI must make the next
step obvious without requiring the user to read the implementation plan.

- **Plan is the front door.** `rpmx bootstrap plan` must print the seed name and
  generation, the effective `source-date-epoch`, every stage with its arch,
  inputs, package count, root/lock/repo paths, lock status (missing/current/
  drifted), and a final `next:` command. `--json` exposes the same data with a
  stable schema version.
- **Every failure is actionable.** Name the stage, architecture, recipe and
  command to retry. A failed stage build must print the exact retry command,
  for example `rpmx bootstrap build bootstrap.yaml --stage stage1 --arch x86_64
  --locked`.
- **No silent overwrite.** A non-empty stage output or root is an error unless
  `--force` is given. Build into a temporary output directory and rename it into
  place only after the stage, repository index and lock all succeed, so a
  failure never leaves a half-valid stage repository.
- **No prompts.** The command is CI-friendly and non-interactive. Destructive
  operations are opt-in flags, not confirmation questions.
- **No raw resolver noise by default.** Show stage progress and summaries; send
  full dnf/podman output to `--verbose` or the log file. `--color` follows the
  existing TTY rules.
- **Validation points to the fix.** An unknown `--stage` lists the available
  stages. A missing `--arch` lists the manifest's arches. A missing `--locked`
  lock says how to create it. An `--offline` cache miss names the stage and the
  online command that would populate it. A `packages: all` error on a seed or
  cross stage suggests an explicit package list. A cycle error names the cycle
  and suggests an `order` edge or `fixed-point` entry. A `source-date-epoch`
  disagreement lists the values and where they came from.
- **Lock workflow is obvious.** `--locked` verifies and fails closed; the error
  must say whether the lock is missing, stale, or drifted, and whether to run
  without `--locked` first or update it intentionally. `--refresh` is the
  explicit way to accept a new resolution.
- **Rebuild is visibly different.** `rpmx bootstrap rebuild` prints the old and
  new root/sysroot lock digests, requires `--srpm-out`, and warns that the input
  SRPM is unchanged. It never silently mutates an existing SRPM.
- **Help is complete.** `rpmx bootstrap --help` includes a short manifest
  example and the plan → build → rebuild flow. `plan --help` links to the
  manifest reference. Documentation tests keep the examples and flags honest.

### 5.7 Tests for stage build

- Unit-test profile synthesis: input repos, extra tools, package union,
  cross/native split, repo-id collision.
- Unit-test the graph order and stage output layout with temporary directories.
- Integration test (`#[ignore]`, or `RPMX_BOOTSTRAP_SMOKE=1`) that builds a
  two-stage ladder from local `file://` repositories, proving:
  - stage outputs are separate repos,
  - stage locks are written,
  - the second stage root contains the first stage's package,
  - `extra-tools` appears only in the declaring stage.
- A `--stage` test that fails cleanly when an input output is absent.
- A `--locked` test that fails with `BOOTSTRAP_006` after a recipe/policy drift.
- UX tests for the plan's `next:` line, unknown-stage/arch errors, missing-lock
  guidance, non-empty-output refusal and atomic stage publication.
- A non-interactive test proving no stage build waits for input.

## 6. Phase 4: fixed point (#2, second step)

Implement `fixed-point` only after the ordered ladder works. The key problem is
that a rebuilt package usually keeps the same NEVRA, so adding the stage output
repo beside the input repo creates duplicate EVRAs.

### 6.1 Repository filtering

Add optional package filters to `Repository`:

```rust
#[serde(default)]
pub exclude: Vec<String>,
```

- Validate names like other package lists.
- Write `exclude=name1,name2,...` into the generated `.repo` file
  (`resolve::write_repositories`).
- Include `exclude` in `closure_digest` only when non-empty, keeping existing
  lock digests unchanged.

### 6.2 Iteration algorithm

1. Pass 0 resolves the stage root from the declared inputs only and builds every
   stage recipe in order. Index the stage output.
2. For iteration `i = 1..max_iterations` (manifest field, default 3):
   - Collect the package names currently present in the stage output.
   - Synthesise a fixed-point profile: the same stage profile, every other
     repository (including the seed) gets
     `exclude = <stage output package names>`, and add the stage output repo.
     This ensures the stage output wins even when NEVRA is unchanged, without
     relying on a release or epoch bump.
   - Resolve and assemble a candidate root under
     `roots/<stage>/<arch>-iter<i>/`; keep its lock/digests.
   - Rebuild only the `fixed-point` recipes in declared/graph order against the
     candidate root, writing over the same stage output.
   - Re-index the stage output.
   - Compare, for each fixed-point recipe, the SHA-512 of every produced RPM,
     plus the candidate root's `closure_digest` and `root_digest`. If both the
     package bytes and the root digests are unchanged from the previous pass,
     promote the candidate root and lock to `roots/<stage>/<arch>`/
     `locks/<stage>/<arch>.lock.yaml` and stop.
3. If not converged after `max_iterations`, return `BOOTSTRAP_005` naming the
   packages whose bytes changed and the last two root digests.

Interpretation of "NEVRAs stop changing": keep NEVRAs fixed and compare
reproducible RPM bytes and root digests. If a distro needs a release bump per
iteration, it can set `--build-release`; the convergence check remains
content-based.

### 6.3 Fixed-point tests

- Pure graph test: a glibc <-> gcc cycle is allowed only when both are in
  `fixed-point`.
- Profile/repo test: `exclude` is rendered into the repo file and changes the
  closure digest only when non-empty.
- Convergence test with a fake builder (or an ignored integration test) that
  reports identical bytes on the second pass and stops.
- Non-convergence test that reports `BOOTSTRAP_005` and the changed packages.

## 7. Phase 5: bootstrap rebuild with a replacement lock (#4)

Keep `rpmx rebuild` strict. Add a separate path.

### 7.1 CLI

```console
rpmx bootstrap rebuild package.src.rpm \
  --root-lock bootstrap/locks/stage1/x86_64.lock.yaml \
  --root bootstrap/roots/stage1/x86_64 \
  --output out/stage1/x86_64 \
  --srpm-out out/stage1/src
```

Optional `--sysroot-lock`/`--sysroot` for a native-cross or foreign-cross SRPM.
The command accepts the same runner/cache/offline/test/signing arguments as
`rebuild`, plus `--replace-root-lock` semantics implicitly (no flag needed:
choosing the bootstrap command is the explicit opt-in).

### 7.2 Implementation

- Add `BootstrapRebuildArgs` and `commands/bootstrap/rebuild.rs`.
- Reuse `rebuild::prepare`/`rebuild::execute` with a new internal
  `RebuildRequest` field:
  `replace_root_lock: bool` and `srpm_out: Option<&Path>`.
- In `prepare`:
  - when `replace_root_lock` is false, keep the existing `ROOT_016` comparison;
  - when true, skip the comparison, still validate the requested lock's target
    and digest, and capture the old embedded root/sysroot for provenance;
  - print `warning("bootstrap", "replacing embedded root lock ... with ...")`.
- After the binary rebuild, re-emit an SRPM:
  - use the extracted `package.yaml` and the unpacked source snapshot;
  - call `srpm::execute` with the replacement root lock, the new sysroot lock
    (if any), the effective `CrossToolchain`, the same cell and policy, and the
    extracted `source-date-epoch`;
  - use the extracted `pipelines/` directory so step digests match;
  - write only to `--srpm-out`; never modify the input SRPM.
- Provenance: add an optional `replaced` section to the SRPM `PackageLock`:
  the old root/sysroot closure and root digests (and optionally the old
  `profile` names). The build log records the same replacement. The new lock's
  `root`/`cross` point at the replacement locks.
- Refuse `--replace-root-lock` (or the bootstrap path) if `--srpm-out` is
  missing, because the acceptance contract requires a new SRPM.

### 7.3 Tests for #4

- Plain `rpmx rebuild --root-lock <mismatched>` still exits 3 with `ROOT_016`.
- `rpmx bootstrap rebuild` accepts a mismatched lock and writes a new SRPM.
- Inspect the new SRPM's `package.lock.json`: root/cross point at the new locks
  and `replaced` records the old digests.
- The input SRPM is byte-identical before and after.
- A missing `--srpm-out` is a recipe error before any build starts.

## 8. Documentation and examples

- `docs/usage/cross-compilation.md`: add a "Native bootstrap" section covering
  `cross.bootstrap`, the recommended explicit `cross.triple` ladder path, the
  native-command fallback, the two roots, `/sysroot` and the link-closure rule.
  Update the current "one profile, one build strategy" text that says a profile
  cannot build the host.
- `docs/usage/bootstrap.md` (new): a quickstart (`plan` → `stage0` → full
  ladder), manifest reference, stage semantics, root paths, `--locked`, fixed
  point, `bootstrap rebuild`, failure recovery, common validation errors, and
  the trust/provenance story. Include the expected `rpmx bootstrap plan` output
  and the exact command to run next.
- `docs/usage/README.md`: add the new guide to the reading order.
- `docs/usage/profiles-and-roots.md`: document stage roots as explicit roots and
  the `requirement` field.
- `docs/usage/reference.md`: add bootstrap error codes, exit codes, environment,
  and limitations.
- `docs/usage/troubleshooting.md`: add "bootstrap stage root not cached",
  "cycle without fixed-point", "replacement lock", "unknown bootstrap stage",
  "architecture required", "stage output already exists" and "locked stage
  lock missing or drifted" entries, each with the fixing command.
- `README.md`: update Status and the command map.
- `CHANGELOG.md`: one entry per phase.
- `examples/bootstrap/`: a three-stage manifest, stage profiles, a small recipe
  set, a checked-in example of `rpmx bootstrap plan` output, and a README
  explaining the plan → build → rebuild flow, failure recovery and which
  network/tooling is required.
- `scripts/bootstrap-smoke.sh`: optional end-to-end smoke gated on
  `RPMX_BOOTSTRAP_SMOKE=1`; do not add it to the default CI job.

Documentation tests in `crates/rpmx-cli/tests/docs.rs` will automatically check
that every new `rpmx bootstrap ...` example and flag exists in `--help`.

## 9. Delivery order and pull requests

1. **PR 1 (#1):** profile marker, validation, native toolchain, lock/SRPM
   provenance, mount-point cache fix, tests, cross-compilation docs.
2. **PR 2 (#2 plan):** manifest model, validation, graph/order, seed-stage
   policy, `source-date-epoch`, seed/generation metadata, `rpmx bootstrap plan`,
   plan/help UX and docs tests, bootstrap guide skeleton.
3. **PR 3 (#2 build + #3):** explicit root pair refactor, stage profile
   synthesis, stage root resolution, ordered stage builds, `extra-tools`/
   `extra-sysroot`, `--locked` stage policy, deterministic repo timestamps,
   seed metadata in stage locks, atomic output and actionable-error UX, docs and
   examples.
4. **PR 4 (#2 fixed point):** `Repository.exclude`, iteration/convergence,
   non-convergence diagnostics, tests.
5. **PR 5 (#4):** bootstrap rebuild, replacement lock provenance, SRPM
   re-emission, tests.

Each PR should keep `cargo fmt`, `cargo clippy --workspace --all-targets -D
warnings`, `cargo test --workspace`, and `cargo deny check licenses` green.

## 10. Acceptance mapping

### Issue #1

- Same-arch `cross` with `bootstrap: true` builds against a sysroot (integration
  smoke; unit-level derivation tests in CI).
- A host/seed library absent from the sysroot fails the existing link-closure
  check (`RPMX-BUILD-009`). Link closure is necessary but not sufficient for
  package-origin isolation; the distro promotion gate must also check the
  installed package set and file ownership.
- The SRPM embeds both locks and `rpmx rebuild` reconstructs the native
  toolchain from `LockCross.native`.
- Non-bootstrap `cross` and `ROOT_006` behaviour is unchanged; covered by
  existing tests plus a new negative test.

### Issue #2

- A three-stage manifest builds from one file (gated integration smoke).
- Stage outputs are separate indexed repositories and stage locks are written
  with deterministic repository timestamps.
- Seed stages and cross stages reject `packages: all`; only the final
  self-hosted stage may use it.
- Stage locks and the plan carry the optional seed/generation metadata.
- A glibc <-> gcc cycle without `fixed-point`/`order` fails with
  `BOOTSTRAP_004` and names the cycle.
- A fixed-point stage converges or fails with `BOOTSTRAP_005` naming the
  changed packages.

### Issue #3

- `extra-tools` appears in the declaring stage's root lock and build log.
- The same recipe builds in a stage with and without the extra tool because the
  recipe model is unchanged.
- Runtime dependencies are untouched; the tool is only a build-root package.

### Issue #4

- A stage 0 SRPM rebuilds in a stage 1 root without source re-assembly.
- The new SRPM embeds the stage 1 lock and records the replaced lock in
  provenance.
- Plain `rpmx rebuild` still rejects a mismatched root lock with `ROOT_016`.
- The input SRPM is unchanged.

### Cross-cutting UX

- `rpmx bootstrap plan` prints the seed name/generation, effective timestamp,
  per-stage inputs, package counts, root/lock/repo paths, lock status and a
  final `next:` command. The `--json` shape is versioned and documented.
- Every common failure names the stage, architecture and fix: unknown stage,
  missing arch, missing/drifted lock, offline cache miss, `packages: all` on a
  seed/cross stage, dependency cycle, `source-date-epoch` disagreement, and
  existing stage output.
- A non-empty stage output is refused without `--force`; stage output is
  published atomically after a successful build, index and lock.
- No command prompts for input; all destructive actions are explicit flags.
- `rpmx bootstrap --help` and `docs/usage/bootstrap.md` contain a working
  quickstart, the plan → build → rebuild flow, and the lock workflow. Docs tests
  keep examples and flags in sync.

## 11. Boundary with the distro repository tooling

This plan deliberately stops at the rpmx engine. The yearly rebootstrap still
needs Eternal-side repository tooling:

- seed manifests and generations (`bootstrap/seeds/*.yaml`,
  `bootstrap/generations/<generation>/...`, `bootstrap/current.yaml`),
- candidate/stable promotion and rollback,
- verification gates beyond link closure (installed package set, file
  ownership, canary builds, image compose/run, NEVRA/file-list/SBOM diff),
- cache/stamp invalidation after a new final self-hosted root is promoted,
- seed adaptation for Fedora/RHEL (package names, entitlements, dnf5 vs dnf4),
- the full-rebuild trigger,
- a separate bootstrap pipeline: path-filtered plan/smoke runs on bootstrap
  changes and a scheduled clean full ladder. Normal release builds consume the
  promoted final self-hosted generation and do not rerun the ladder on every
  build.

rpmx should provide the data those tools need: deterministic stage repository
metadata, self-describing stage locks, root/sysroot digests, and a stable CLI.
It should not own promotion policy.

### Package identity and repository layout

Bootstrap reuses the normal recipes and package names. Do not fork
`glibc-bootstrap`, `gcc-bootstrap` or similar recipes: a fork would stop testing
the packages that actually ship. Each stage is a separate indexed repository,
and normal `rolling` builds consume only the promoted final self-hosted root.

Do not add an automatic bootstrap EVR suffix in v1. Separate repositories keep
bootstrap and final builds from competing, so
`glibc-...-1.eternal+buildN` keeps the same name and EVR scheme in every stage.
If a future consumer needs bootstrap and final packages in one repository, add a
`release-suffix` field that inserts `~bootstrap` before `+buildN` (Fedora's
convention); do not use a dist tag or a dot suffix, which do not have the
intended pre-release ordering.

Package-origin verification is a specific gap. Link closure is necessary but
not sufficient for "no seed packages": a seed library with the same SONAME can
still satisfy `DT_NEEDED`. The promotion gate must check the installed package
set (`rpm -qa` in the final self-hosted root), critical file ownership (for
example `/usr/lib64/libc.so.6` comes from the Eternal glibc), and canary
builds. A small `rpmx root packages`/`rpmx root diff` helper could make that
easier later, but the gate itself stays outside rpmx.

## 12. Risks and open questions

- **Seed contamination in same-arch builds.** ELF machine and `DT_NEEDED`
  checks catch a library missing from the sysroot, but they cannot distinguish
  a seed library from a sysroot library with the same SONAME. The promotion
  gate must verify package origin and file ownership; a future
  `rpmx root packages`/`root diff` helper can assist but must not replace the
  gate.
- **Repo priority vs. `exclude`.** The fixed-point design relies on excluding
  input packages that the stage output supersedes. If a distro needs multiple
  providers or partial filters, add repository priority or a filtered repo view;
  do not rely on dnf ordering.
- **Determinism of stage repo metadata.** Now part of the schema: `repo::index`
  uses the stage/manifest `source-date-epoch`, or the common recipe epoch when
  neither is set. If recipes disagree and no epoch is declared, bootstrap fails
  rather than silently using wall-clock time.
- **Multi-cell recipes.** V1 rejects them in bootstrap stages. Supporting them
  means ordering and selecting by cell, not just by recipe/package name.
- **Credentials.** The effective stage profile must stay in memory or in the
  work directory; only root locks (which strip auth) belong under `locks/`.
- **QEMU/podman crosses.** The bwrap backend mounts the sysroot; the podman
  backend currently has no sysroot handling. Keep bootstrap/cross builds on
  bwrap and reject `--runner podman` for a cross stage with a clear error.
