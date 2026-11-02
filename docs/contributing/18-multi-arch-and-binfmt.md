# Multi-architecture builds and binfmt execution

Status: proposed implementation plan, 2026-11-02. Revised after four independent
reviews (correctness, second opinion, security, spec fidelity).

## 1. Goals

1. Teach the existing bubblewrap runner to execute foreign targets through
   qemu-user/`binfmt_misc`. Binfmt is an execution capability of the bwrap
   sandbox, not a separate runner.
2. Make `rpmx build` accept several target architectures and build them
   concurrently.
3. Make the result correct: per-arch roots, outputs, locks, evidence and
   errors, with no shared-state races.
4. Make the default foreign path bubblewrap + binfmt for userspace builds. The
   qemu-system VM remains a first-class, explicit runner for kernel-facing,
   high-assurance, or target-kernel builds.
5. Keep the codebase maintainable: one source of truth for runner names and
   selection, one build-unit abstraction, no duplicated plan loads, no
   read-modify-write evidence files, and runner assets decoupled from the
   build-root cache identity.

## 2. Non-goals

- Cross-compilation. Both foreign modes execute target binaries.
- Parallelising matrix *cells* within one architecture. The scheduler is
  generic so this can be added later, but it is out of scope.
- Multi-arch `rpmx test`, `rpmx source`, `rpmx srpm`, `rpmx plan` and
  `rpmx lint` in the first cut. `test` is the natural follow-up; `rebuild` is
  single-SRPM and stays single-arch.
- Removing the qemu-system runner. It remains the explicit hardened path for
  kernel-facing, target-kernel and high-assurance builds.
- Sharing one source snapshot across arches. Source assembly can be
  arch-dependent through templates, so it stays per-arch.
- Redesigning root-assembly scriptlet isolation (a pre-existing issue, see
  §7). This plan documents it and adds a follow-up ticket, but does not change
  it.

## 3. Current state and corrections to the first draft

- The tree currently **does compile** (`cargo check --workspace` is clean).
  The earlier `color`/`dispatch` mismatch has been fixed in the working tree.
- The working tree is **dirty** with in-progress work across CLI, runner, docs
  and tests. Implementation must start from a committed baseline or a dedicated
  worktree; do not overwrite those changes. Ticket 0.1 is "confirm/commit the
  baseline", not "fix a known compile error".
- `RunnerKind` has `Auto`, `Bwrap`, `Podman`, `Qemu`
  (`crates/rpmx-runner/src/runner/mod.rs:137-160`).
- `Auto` currently selects bubblewrap native / qemu foreign
  (`runner/mod.rs:222-245`). This plan changes `Auto` to bubblewrap for both,
  using binfmt for a foreign target; qemu becomes explicit.
- `BwrapSession` is a reusable bwrap command builder around a root
  (`runner/backend/bwrap.rs:27-100`).
- The project previously shipped a qemu-user runner and replaced it with
  qemu-system (`43d1c97`, `b4ad3fe`). The old `EmulatedRunner` was a thin
  wrapper around `BwrapSession` with the target root.
- `build_from_recipe` resolves one arch, one plan, one root, one pipeline
  (`crates/rpmx-cli/src/commands/build.rs:165-340`).
- `BuildPipeline::execute` assembles source once per arch, then loops cells
  sequentially (`commands/pipeline.rs:52-160`).
- `autoroot` serialises publishers with a per-key `CacheLock`
  (`commands/autoroot.rs:200-280`), but the requirement digest folds in the
  appliance flag and does not reliably include the requested arch.
- The reviewable lock is per-arch (`commands/lock.rs:29-38`, `:220-300`).
  The SRPM-embedded lock is a **different document**
  (`rpmx-store/src/srpm.rs:275-290`) and is not changed by this plan.
- `--junit` is written inside `rebuild::tests::run` before returning a test
  error (`commands/rebuild/tests.rs:121-151`).
- `--sbom FILE` is one path reused for every cell and arch
  (`commands/rebuild/mod.rs:257-280`).
- `--arch` is a single `String` on every command
  (`commands/args.rs:59-60` and four more copies).
- `README.md:44` is stale ("aarch64 builds run through qemu-user binfmt").
  `docs/contributing/09-sandboxing.md:57` is a **melange** reference and is
  correct; it must not be "fixed".

## 4. Contracts

### 4.1 CLI surface

```text
rpmx build [FILE] --arch <ARCH>[,<ARCH>...] [--arch all] [--arch-jobs N|all]
rpmx build ... --runner auto|bwrap|podman|qemu
rpmx lock [FILE] --arch <ARCH>[,<ARCH>...] [--arch all]
```

- `--arch` is repeatable and comma-separated, validated by clap through
  `Arch::parse`. Default: native arch.
- `--arch all` is an explicit user request. It expands to the intersection of:
  - `Arch::ALL`;
  - the effective root profile's `archs`, where "effective" includes a
    recipe-named profile, `--profile`, or an inline root with `archs`;
  - `package.architectures.allow` minus `package.architectures.deny`.
- A profile `archs` list is an allowed set, not a single root. Profiles may
  list several arches; the resolver derives a single-arch profile for the
  requested target.
- An explicitly requested arch outside the profile's allowed set or outside
  `package.architectures` is a usage error. An arch skipped only because of
  `all` is a warning.
- Duplicate arches are a usage error.
- `--arch all` with `--runner auto` is allowed, but the preflight prints the
  foreign arches and their selected runners before starting. This preserves
  the "foreign execution is explicit" rule: the user asked for all.
- `--runner bwrap` means "bubblewrap sandbox: execute native targets directly,
  foreign targets through host binfmt". This is the same runner for both; the
  execution mechanism is chosen by the host/target pair. A foreign target with
  no usable binfmt handler is an error, not a silent qemu fallback.
- `--runner qemu` means qemu-system for every target, including native.
- `auto` means bubblewrap native, bubblewrap + binfmt foreign. It never starts
  a VM. A foreign target with no usable binfmt handler fails early with
  `RPMX-RUNNER-002` and points at `--runner qemu` or `qemu-user-static`.

### 4.2 Output layout

`--output ROOT` is the artifacts root. Every arch writes to `ROOT/<arch>/`.
The default is `packages`, so the default stays `packages/<arch>`.

This is a breaking change for `--output out` with one arch: it now writes to
`out/<arch>/`. It is accepted because multi-arch is first-class and a
conditional "exact for one arch, root for many" contract is worse. The docs
(README `repo --rpms ./out` example, `docs/usage/building.md`) must be updated
in the same ticket that changes the behaviour.

`--stage` stays single-arch and is rejected with more than one arch.

### 4.3 Side artifacts

- `--junit FILE` remains one file. Test cases from every arch and cell are
  collected in memory and written once, **including outcomes from an arch that
  failed after its tests ran**. `TestOutcome` gains `arch` and `cell`; the JUnit
  `classname` is `<arch>/<cell>/<stage>`.
- `--sbom FILE` becomes `--sbom-dir DIR`. The default keeps the documented
  name `<name>-<evr>.build.spdx.json` inside the arch output directory; an
  explicit shared `--sbom-dir` writes `<name>-<evr>.<arch>.build.spdx.json`.
  This also fixes the existing multi-cell overwrite bug.
- `--root`, `--root-lock`, `--test-root-lock` and `--stage` are rejected when
  more than one arch is requested, naming the offending flag.
- `--no-index`, `--no-lint` and `--cell` apply to every arch.

### 4.4 Concurrency

- `--arch-jobs N` bounds concurrent architecture builds. `all` means one worker
  per arch. Default `auto`:
  - `auto` = `min(arch_count, available_parallelism)` for bwrap/binfmt;
  - `auto` = `1` for an explicit `--runner qemu`, because each worker is a VM.
  The resolved value is printed in the preflight line.
- `--jobs` remains per-step parallelism. When it is not given, the default is
  `max(1, available_parallelism / arch_jobs)`, so `--arch-jobs auto` does not
  oversubscribe the host. An explicit `--jobs` is used verbatim, and the
  preflight warns when `arch_jobs * jobs` exceeds the available parallelism.
- The pool is `std::thread::scope` plus a work queue. No async runtime and no
  new dependency.
- Preparation (recipe parse, arch validation, plan expansion) is sequential and
  fails before any worker starts. Root resolution and execution run in workers.
- All arches are attempted. A failure in one does not cancel the others. The
  command exits non-zero with the **first failure in arch order**, and every
  failure is reported in human and JSON output.
- A worker panic is caught and converted to an internal error; it does not
  abort the other workers.
- Ctrl-C must not orphan qemu VMs. Adaptive qemu children get
  `PR_SET_PDEATHSIG` (Linux) or an explicit signal handler; bwrap already uses
  `--die-with-parent`; podman uses `--rm`.

### 4.5 Lock schema 2

Only the **reviewable** `package.lock.json` (`commands/lock.rs::PackageLock`)
moves to schema 2. The SRPM-embedded lock
(`rpmx-store::srpm::PackageLock`) is a different document, stays schema 1, and
`rebuild` is unaffected. Add a regression test that a pre-change SRPM still
rebuilds.

```json
{
  "schema": 2,
  "recipe_digest": "...",
  "recipe_files": [],
  "context": null,
  "source_date_epoch": 1700000000,
  "policy": { "dist_tag": ".fc44" },
  "sources": [],
  "archs": {
    "x86_64": { "root": { ... }, "cells": [ ... ] },
    "aarch64": { "root": { ... }, "cells": [ ... ] }
  }
}
```

- `LockedRoot.arch` is removed; the map key is the arch.
- `sources`, `recipe_digest`, `context`, `source_date_epoch` and `policy` are
  shared. `cells` and `root` are per-arch.
- Bare `rpmx lock` rewrites the arches already recorded in the lock, or the
  native arch if there is no lock. `rpmx lock --arch A,B` writes exactly A,B;
  unrequested arches are dropped, so no stale entry can survive a shared-field
  change.
- `rpmx lock --check` with no `--arch` verifies every recorded arch; with
  `--arch` it verifies that subset and ignores the rest.
- `build --locked` verifies every requested arch. An arch absent from the lock
  is an error, not drift.
- Schema 1 reviewable locks are rejected with "regenerate with the current
  rpmx". There are no users and no migration code.

### 4.6 Logging contract

Every build-related log line carries the target architecture, including
single-arch builds. This is a deliberate output change; there are no users and
consistent lines are easier to grep and to interleave safely.

- Canonical shape: `rpmx: <phase> [<arch>] <detail>`.
  Examples:
  ```text
  rpmx: resolve  [x86_64]  fedora-44 x86_64 209 packages (cache hit)
  rpmx: build    [aarch64] 12 steps
  rpmx: step     [aarch64] build:runs#1
  rpmx: wrote    [aarch64] packages/aarch64/hello-0.1.0-1.fc44.aarch64.rpm
  rpmx: test     [aarch64] installed smoke: ok (1.2s)
  ```
- The arch is part of the log scope, not concatenated at each call site.
  `rpmx-base::output` owns one formatter.
- Guest stdout/stderr pumped through the runner is prefixed with the same
  `[<arch>]` tag. The pump thread receives the scope explicitly; a new thread
  must not inherit an empty thread-local.
- Lines that are not architecture-scoped (for example `rpmx lint`) omit the
  tag.
- `--json` stdout stays exactly one document; the tag is stderr-only. JSON
  artifacts and diagnostics carry an `arch` field instead.
- Tests: every line from a two-arch build contains the right tag; no line is
  mislabelled; guest output from one arch never appears under the other; a
  single-arch build still includes its arch.

## 5. Architecture

### 5.1 Correctness prerequisites

These are real defects that multi-arch and binfmt would turn into
silent-wrong-result bugs. They land before binfmt execution is enabled.

1. **CAS publication is not thread-safe.**
   `rpmx-store/src/cas.rs:48-59` writes `target.tmp.<pid>`. Two workers in one
   process publishing the same digest race on the temporary file. Fix with a
   unique suffix (`pid` + atomic counter or `tempfile::NamedTempFile`) and a
   concurrent same-digest publication test.

2. **A cache hit does not check the target arch.**
   `autoroot::load_cached` (`commands/autoroot.rs:517-558`) calls
   `RootSelection::verify` only when the root is newer than the lock. A normal
   hit skips `require_target`. Always call `document.require_target(arch)` on a
   hit; keep the expensive digest walk conditional.

3. **Profiles reject multi-arch and ignore the requested arch.**
   `Profile::validate` rejects `archs.len() > 1`
   (`rpmx-root/src/profile.rs:192-196`), and `autoroot::requirement` only fills
   `archs` when empty (`commands/autoroot.rs:116-118`). Requesting aarch64 with
   an x86_64 profile silently resolves an x86_64 root. Fix:
   - `profile.archs` is an allowed set;
   - validate the requested arch against it;
   - derive a single-arch profile (`archs = [target]`) before hashing,
     validating and resolving;
   - add tests for named, inline and `--profile` roots.
   - Direct consumers must not use `Profile::arch()` (first entry) once a
     profile may list several arches. `rpmx root resolve` and
     `rpmx-root/src/root/resolve.rs` need an explicit target (`--arch`) or must
     keep the low-level profile single-arch by deriving it at the boundary.
     Add a test with `archs: [aarch64, x86_64]` and `--arch x86_64`.

4. **The appliance is part of the build-root cache identity.**
   `requirement()` folds `needs_appliance` into the digest
   (`commands/autoroot.rs:129-139`), and `lock::build` computes it
   runner-blind (`commands/lock.rs:225`). Under `--runner bwrap` on a foreign
   target the root requirement differs from the qemu one, so `--locked` reports
   spurious drift.
   This also contradicts invariants I1/I3 in
   `docs/contributing/16-qemu-appliance.md:171-173` and the rule that transport
   is a runner artifact. Fix by decoupling:
   - the build-root requirement digest depends only on recipe/profile/target;
   - the root cache entry stores only the build root;
   - the qemu appliance is resolved and cached separately, keyed by the root
     lock plus appliance schema;
   - `rebuild` resolves the appliance from the SRPM's embedded root lock and
     the separate appliance cache. It must not require the appliance to be
     embedded in the root lock, because an SRPM may have been produced by a
     binfmt build and later rebuilt with `--runner qemu`. A post-1.4 SRPM
     rebuilt with qemu is the regression test.

5. **`CURRENT_PHASE` is process-global.**
   `rpmx-base/src/output.rs:24,120-143` uses
   `Mutex<Option<&'static str>>`; concurrent workers will restore each other's
   phases and mislabel guest output. Convert phase and arch scope to
   thread-locals. Guest-output pump threads are spawned inside the runner
   (`runner/backend/process.rs:60-67`); capture the phase/arch context before
   spawning and pass it explicitly, because a new thread starts with an empty
   thread-local scope.

6. **qemu port allocation and orphan cleanup.**
   `free_port` (`qemu/disk.rs:270-286`) closes the listener before qemu binds;
   two workers can pick the same port. Holding the listener while qemu binds
   causes a direct collision, and releasing it allows another worker to take
   the port before qemu binds. Implement bounded bind-retry on
   `RUNNER-004`/port-in-use (regenerate port-dependent known-hosts data), or a
   real socket handoff; a single reservation is not sufficient. qemu also has
   no `PR_SET_PDEATHSIG`; Ctrl-C leaves VMs running. Add `pre_exec` PDEATHSIG
   and test the concurrent-start path.

### 5.2 Runner layer

No new `RunnerKind`. Binfmt is a capability of the existing bubblewrap runner;
the runner abstraction stays "how to create an execution environment", and
bubblewrap remains the sandbox. This matches melange's model, where Linux
`--runner bwrap` executes foreign steps through host binfmt.

- `BwrapBackend::supports(host, target)` becomes:
  - `host == target`; or
  - `host != target && root.is_some() && emulation_available(target)`.
- `choose(Bwrap)` becomes:
  - native, root or host root: `Selection::Bwrap(root)`;
  - foreign with a root and a usable handler: `Selection::Bwrap(root)`;
  - foreign with no root: `RPMX-RUNNER-002` ("bubblewrap needs an assembled
    target root for a foreign target");
  - foreign with no usable handler: `RPMX-RUNNER-002` naming the missing
    handler and pointing at `--runner qemu` / `qemu-user-static`.
- `choose(Auto)` uses the same rules as `choose(Bwrap)`: native targets use
  bwrap directly; foreign targets use bwrap + binfmt when a usable handler
  exists; otherwise `RPMX-RUNNER-002`. `Auto` never selects qemu.
- `BwrapSession` already bind-mounts the root as `/` and runs the command; no
  change is needed to the bwrap argument list. The only difference is that the
  root contains foreign binaries and the host kernel translates their `exec`.
- `choose` becomes pure and takes `emulation_available: bool` (probed once
  during sequential preparation) so all branches are unit-testable.
- `emulation_available` must require the `F` flag. Without `F`, the kernel
  resolves the interpreter path in the exec'ing mount namespace, where the
  bwrap guest root is `/` and the host interpreter is absent. Report the exact
  reason. Bind the registered interpreter read-only into the sandbox at its
  registered path as defence in depth, and record its path and digest in
  evidence.
- `appliance::Policy::Auto::should_resolve` returns false for `Bwrap` and
  `Auto`; only explicit `Qemu` needs an appliance. Update
  `docs/contributing/16-qemu-appliance.md` D4/§6.2 to key on the selected
  backend, not on `target != host`.
- `doctor` reports binfmt availability as a bubblewrap capability: handler
  flags and interpreter path, not as a separate runner.
- Evidence records `runner = bubblewrap` and `execution = foreign-binfmt`
  (plus interpreter path/digest), so an operator can distinguish a native bwrap
  build from a binfmt build.

If a hard, visible opt-in is preferred over "bwrap implies binfmt for foreign",
the alternative is a composite value `bwrap-binfmt` that selects the same
backend with the same rules. It should not be a second backend; it is only a
spelling. The recommended surface is `--runner bwrap`, because the sandbox is
what the user is choosing.

### 5.3 Planner layer

Replace `load_plan(file, arch, ...)` with:

```rust
pub struct LoadedRecipe {
    bundle: RecipeBundle,
    step_root: Option<PathBuf>,
    dist_tag: Option<String>,
}

impl LoadedRecipe {
    pub fn load(file: &Path, pipeline_dir: Option<&Path>, policy: ...) -> Result<Self>;
    pub fn plan(&self, arch: &str, ...) -> Result<Plan>;
}
```

- Removes the `Box::leak` for `dist_tag`.
- `SourceRequest`/`SrpmRequest` take `&RecipeBundle` + `&Plan`, so one arch
  loads and expands once instead of three times.
- `BuildPipeline` holds `bundle`, `plan`, `arch`, `output`, step settings.
- Effective policy must be reconciled with the root lock **before** source
  assembly. Today `srpm::execute` reconciles the root lock's dist-tag after
  `source::execute` has already planned with the CLI/profile policy
  (`commands/srpm.rs:105-121`). That is a latent mismatch. Resolve the root,
  reconcile policy, then re-expand the plan once; use that plan for source,
  SRPM and tests. Add a regression test with a root lock dist-tag that differs
  from the CLI/profile.
- All existing CLI commands migrate to `LoadedRecipe`.

### 5.4 Build layer

```rust
struct PreparedBuild<'a> {
    arch: Arch,
    output: PathBuf,
    bundle: &'a RecipeBundle,
    plan: Plan,
    policy: PlanPolicy<'a>,
    args: &'a BuildArgs,
}

struct ArchOutcome {
    arch: Arch,
    rpms: Vec<PathBuf>,
    srpms: Vec<PathBuf>,
    evidence: Vec<PathBuf>,
    repository: Vec<PathBuf>,
    tests: Vec<TestOutcome>,
    error: Option<Error>,
}

impl PreparedBuild<'_> {
    fn execute(self) -> ArchOutcome;
}
```

`execute` never discards partial results. Artifacts and test outcomes produced
before a failure stay in `ArchOutcome`; `error` carries the terminal failure.
This is what makes JUnit and JSON complete when one arch fails.

`build_from_recipe`:

1. Load `LoadedRecipe` once.
2. Resolve and validate the arch list (profile, package allow/deny, duplicates).
3. For each arch: expand the plan, select cells, resolve the output dir.
   Defer all `--test` **name-membership** checks until after root resolution
   and policy reconciliation. Preparation may only reject an empty or malformed
   flag list; it must not reject a name that the reconciled policy could
   produce. Add the `smoke${profile.dist_tag}` / `--test smoke.fc44` regression
   case. Collect `PreparedBuild`s; any fatal preparation error aborts before
   execution.
4. Prepare sequentially; run workers through the pool.
5. Flatten `ArchOutcome`s in arch order.
6. Write JUnit once from all outcomes, including failed arches.
7. Render exactly one final report/JSON envelope with all artifacts and
   diagnostics, then return the first error marked `reported`.

`PreparedBuild::execute` runs root resolution, policy reconciliation, source,
SRPM, rebuild, index and tests, and returns `ArchOutcome`. `repo::index` runs
per arch on its own directory.

Each `PreparedBuild` owns a private `WorkDir`, including test-root paths. Two
arches must never share `work/test-root`, `work/test-resolve`,
`work/test-assemble` or a root mount.

### 5.5 Scheduler

New module `crates/rpmx-cli/src/commands/parallel.rs`:

```rust
pub fn run_parallel<U, O, F>(
    units: Vec<U>,
    limit: NonZeroUsize,
    run: F,
) -> Vec<(usize, O)>
where
    U: Send,
    O: Send,
    F: Fn(U) -> O + Sync,
```

- `std::thread::scope`, `Mutex<vec::IntoIter<U>>`, `mpsc`.
- Each result carries its input index, so arch order is restored.
- `catch_unwind` around each unit; a panic becomes an internal `ArchOutcome`
  error and does not kill the pool.
- Unit tests use a barrier and an atomic counter to prove the limit, all-run
  after failure, panic isolation, and deterministic ordering.
- No new dependency.

### 5.6 Output and errors

- `rpmx-base::output`: phase and arch scope become thread-locals. `phase`
  renders `rpmx: <phase> [<arch>] <detail>` when scoped. Single-arch output is
  unchanged without a scope. Capture the phase/arch context before spawning a
  guest-output pump thread and pass it explicitly; a thread-local alone would
  leave guest lines unattributed (`runner/backend/process.rs:60-67`).
- `Error` gains `related: Vec<Error>` (skipped in serialization) and
  `aggregate`. `main.rs` emits every diagnostic in the JSON envelope and prints
  every human-readable failure. Exit code is the first error in arch order.
- **Exactly one JSON document per command.** The final `build` envelope must
  contain `ok`, the artifacts produced by every arch, and every diagnostic,
  including partial artifacts from a failed arch. The returned error must be
  marked `reported` (or the command must render the envelope itself) so
  `main.rs` does not append a second error document. Add a test that
  `build --json` with one failing arch parses as exactly one JSON value and has
  `ok: false`, both successes' artifacts and both diagnostics.
- `report::ArtifactGroup` gains `arch: Option<&str>`; JSON artifact entries gain
  `"arch"`. The envelope stays schema 1 because the field is additive.

## 6. Work plan

Each ticket is independently shippable and must keep `cargo fmt`, `cargo
clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and
`cargo deny check licenses advisories` green.

### Review gates

Every implementation slice gets a scoped agent review before it is considered
done. The review is part of the ticket, not a final step.

- **Always:** `reviewer` (correctness, regressions, missing tests) and
  `reviewer-tests` (can the tests fail; do they assert behaviour). Run them on
  the slice diff, not the whole repository.
- **When the slice touches root resolution, binfmt, bwrap, qemu, signing or
  untrusted input:** `reviewer-security`.
- **When the slice changes a CLI, lock, output or evidence contract:**
  `reviewer-open` (contract drift, resource limits, failure paths).
- **When the slice changes documented behaviour or the plan:** `reviewer-spec`
  (fidelity to `docs/contributing` and `docs/usage`).
- Each reviewer receives: the slice diff, the ticket acceptance criteria, and
  the relevant files. Findings are fixed in the same slice or explicitly
  waived with a recorded reason. The review outcome (reviewer, findings,
  resolution) goes in the commit message or the PR description.
- Do not run the whole reviewer catalogue on every slice; pick the scoped set
  above. A slice with no security-relevant change does not need
  `reviewer-security`.
- The final slice (docs/evidence) gets a consistency review across the whole
  diff.

### Phase 0 — baseline and contract

0.1 Confirm/commit the current working tree, or create a dedicated
worktree/branch. Do not overwrite in-progress edits.
- Proof: `cargo check --workspace`, `cargo test --workspace`.

0.2 Write `docs/contributing/18-multi-arch-and-binfmt.md` with the contracts
from §4, the runner table, the binfmt/VM tradeoff, and the decision that
architecture is a CLI build-target axis, not a matrix axis (close open question
4 in `11-matrices.md`).
0.3 Fix `README.md:44`; do **not** change `09-sandboxing.md:57` (it describes
melange correctly). Update `09-sandboxing.md` §7 line 385 if binfmt's status
changes. Update the other places that promise qemu for `auto`:
`README.md:336` ("A foreign target uses qemu-system automatically"),
`README.md:41-58` (requirements: binfmt is mandatory for foreign; qemu-system is
the explicit VM path), `docs/usage/getting-started.md:37`, and
`docs/contributing/09-sandboxing.md:297` (`auto` list).
- Proof: docs review; `cargo test -p rpmx --test docs`.
- Review gate: `reviewer` + `reviewer-spec` on the contract/doc diff.

0.4 Resolve the `cargo-deny` baseline before code slices start.
- `cargo deny check licenses` passes after removing the two invalid
  non-SPDX entries from `deny.toml` (`MIT/Apache-2.0`, `Apache-2.0 / MIT`);
  `MIT` and `Apache-2.0` remain individually allowed.
- `cargo deny check advisories` currently fails on four pre-existing findings:
  `crypto-bigint` RUSTSEC-2026-0387, `ml-dsa` RUSTSEC-2025-0144, `rsa`
  RUSTSEC-2023-0071 (all through `pgp`), and `unsafe-libyaml`
  RUSTSEC-2024-0449 (through `serde_yaml_ng`).
- Decide explicitly: targeted `[advisories] ignore` entries with a recorded
  rationale and expiry/review date, dependency upgrades/replacements, or a
  separate hardening ticket. Do not silently ignore the crypto findings.
- Proof: `cargo deny check licenses` passes; the advisory decision is recorded
  in `deny.toml`, the issue tracker, or the plan.

### Phase 1 — correctness prerequisites (no new feature)

1.1 CAS unique temporary files + concurrent publication test.
1.2 Root cache arch validation (`require_target` on every hit).
1.3 Profile arch sets + per-target profile derivation + tests.
1.4 Decouple appliance from root requirement/cache identity; add
`ensure_appliance` as a separate step; update `16-qemu-appliance.md` D4/§6.2.
1.5 Thread-local phase/arch output scope.
1.6 qemu `free_port` listener lifetime and `PR_SET_PDEATHSIG`.
1.7 `Error::related`/`aggregate` + JSON diagnostics.
- Proof: unit tests for each; existing workspace tests; a two-arch root-cache
test asserting distinct `lock.archs`; a concurrent CAS test; a qemu
concurrent-start test.
- Review gate: `reviewer` + `reviewer-tests` + `reviewer-security` (root
identity, CAS, qemu lifecycle) on the Phase 1 diff.

### Phase 2 — foundation refactors (no behaviour change)

2.1 Centralise runner names (`RunnerKind::ALL`, `names()`, one clap parser);
replace the five hardcoded lists in `commands/args.rs`.
2.2 `LoadedRecipe`; migrate `load_plan` callers; pass `&RecipeBundle`/`&Plan`
through source/srpm; remove `Box::leak`.
2.3 Reconcile effective policy before source assembly; one plan for source,
SRPM and tests; regression test for a root-lock dist-tag.
2.4 Evidence aggregation: `TestOutcome { arch, cell, stage }`; `tests::run`
returns outcomes; `PipelineOutput` carries them; `write_junit` writes once.
2.5 Output/SBOM path helpers.
- Proof: existing source/srpm/build tests; multi-cell JUnit test; policy
reconciliation test.
- Review gate: `reviewer` + `reviewer-tests` + `reviewer-open` on the
plan/evidence contract diff.

### Phase 3 — bubblewrap + binfmt

3.1 Extend `BwrapBackend`/`choose` to support foreign targets through binfmt;
make `auto` select bwrap for foreign; doctor capability; pure `choose`;
`F`-flag check; interpreter binding and evidence. No new `RunnerKind` or
backend.
3.2 `appliance::should_resolve` false for `Bwrap` and `Auto`; only explicit
`Qemu` resolves one. Docs and threat-model update: the VM is the boundary when
qemu is selected, not the default.
3.3 End-to-end binfmt proof on x86_64:
- aarch64 build with no `--runner` (default `auto`) and with `--runner bwrap`,
  build- and installed-stage tests;
- on a host with `qemu-system-aarch64`, the same recipe with `--runner qemu`;
  compare RPM payloads (name, arch, file list, file digests, build-id), not
  signatures/timestamps;
- record the result in the design doc.
- Manual/CI-optional because `ubuntu-latest` has no binfmt handler.
- Proof: unit matrix for `choose`; `RPMX-RUNNER-002` before execution when the
  handler is missing or lacks `F`; native target uses bwrap without binfmt;
  foreign target runs a step through binfmt.
- Review gate: `reviewer` + `reviewer-tests` + `reviewer-security` +
  `reviewer-open` on the runner/policy diff.

### Phase 4 — serial multi-arch build

4.1 CLI and arch resolution: repeatable/comma `--arch`, `--arch all`,
`Arch::ALL`, clap validation, duplicates, profile/package allow-deny,
`package.architectures` model + validation.
4.2 Per-arch build units, serial execution, output root semantics, rejection of
incompatible flags, per-arch indexing/report, `--sbom-dir` and JUnit
aggregation (so there is no window where multi-arch writes collide), docs
updates in the same ticket.
4.3 Lock schema 2: `lock`, `lock --check`, `build --locked`; clarify the
embedded SRPM lock is unchanged; schema-1 rejection; add an old-SRPM rebuild
regression test.
- Proof: CLI parsing tests; fake `PreparedBuild` aggregation test; real
two-arch serial build (x86_64 + aarch64) with two output roots/repodata and no
cross-arch files; rejection tests; lock round-trip/drift/subset tests.
- Review gate: `reviewer` + `reviewer-tests` + `reviewer-open` +
  `reviewer-spec` on the CLI/lock/output diff.

### Phase 5 — concurrent execution

5.1 `run_parallel` + tests.
5.2 Wire `--arch-jobs` (default `auto`: `min(arch_count, available_parallelism)`
for bwrap/binfmt, `1` for explicit qemu; `all` = arch count); preparation
sequential, execution parallel; thread-local scopes; error aggregation.
5.3 Resource preflight: `arch_jobs * jobs` warning; qemu memory/disk estimate
(2 GiB and one root copy per VM **per stage**); cap `all` when it exceeds a
memory-derived bound.
5.4 Ctrl-C cleanup: PDEATHSIG on qemu; no orphan VMs.
- Proof: barrier/atomic scheduler tests; panic isolation; `--arch-jobs 1`
  byte-identical to Phase 4; `--arch-jobs 2` overlap test with a fake step;
  two arches with installed-stage tests have distinct test roots;
  concurrent qemu start test.
- Review gate: `reviewer` + `reviewer-tests` + `reviewer-open` +
  `reviewer-security` (qemu cleanup, panic isolation) on the scheduler diff.

### Phase 6 — polish and documentation

6.1 Verify `--sbom-dir` and JUnit aggregation across arches; add the
failed-arch JUnit/JSON evidence tests.
6.2 Record execution mode in build evidence: runner kind, binfmt interpreter
path/digest/handler flags, or qemu appliance digest. Add to the sandbox-config
digest in `RPMX-MANIFEST.json` where applicable.
6.3 `rpmx plan --arch a,b` and `rpmx lint --arch a,b` (optional but useful).
6.4 Docs: README, `docs/usage/building.md`, `docs/usage/reference.md`,
`docs/usage/getting-started.md`, `docs/contributing/09-sandboxing.md`,
`16-qemu-appliance.md`, `11-matrices.md`, `CHANGELOG.md`.
6.5 CI/manual harness: `scripts/check-multiarch.sh`; optional CI job only when
binfmt/qemu are available.
- Review gate: `reviewer` + `reviewer-spec` + a whole-diff consistency review
  across all earlier slices.

## 7. Risks, security and mitigations

| Risk | Mitigation |
|---|---|
| binfmt silently runs the wrong arch root | Phase 1.2/1.3 make the arch part of root identity and validate every cache hit; explicit wrong-arch request errors. |
| CAS race corrupts a shared blob | Unique temp names + atomic rename + concurrent test. |
| qemu port collision under parallel starts | Bind-retry or socket handoff; concurrent-start test. |
| Ctrl-C leaves 2 GiB VMs | PDEATHSIG / signal handling; test cleanup. |
| Background JUnit/JSON loss on failure | `ArchOutcome` retains partial evidence; write once after join. |
| Lock/bwrap-binfmt appliance drift | Decouple build-root identity from runner assets; `--locked` never depends on the runner. |
| N VMs exhaust memory/disk | Only an explicit `--runner qemu` starts VMs; `--arch-jobs` defaults to 1 for qemu; preflight; docs state per-stage VM cost. |
| binfmt handler without `F` | Require `F`; bind interpreter read-only; record interpreter. |
| binfmt is a weaker boundary than the VM | It is the default, matching native bwrap's isolation class; `--runner qemu` is the explicit VM path. Record execution mode in evidence; document that kernel-facing and high-assurance builds should use qemu or a native builder. |
| Root-assembly scriptlets run outside the VM with SYS_ADMIN and seccomp=unconfined | **Pre-existing** (`root/container.rs:145-182`). Add a follow-up ticket to reduce capabilities or move assembly into the VM; document the boundary in `08-threat-model.md`. Not a regression from binfmt, but multi-arch multiplies it. |
| Host has binfmt but not for the requested arch | Exact handler check; fail `RPMX-RUNNER-002` before execution. |
| `--arch all` starts foreign builds unexpectedly | `all` is explicit; preflight names foreign arches and their execution mode (`bwrap+binfmt`). |
| Two arches write the same file | Output always sharded by arch; duplicate arch list rejected. |
| Test roots collide | Each `PreparedBuild` owns a private work tree; test asserts distinct test-root locks. |
| Log interleaving | Thread-local phase/arch scope; one write per line. |

## 8. Effort

- Phase 0: 0.5 day (plus whatever is needed to land the current dirty tree).
- Phase 1: 2–3 days (root/arch correctness and appliance decoupling are the
  expensive part).
- Phase 2: 1.5–2 days.
- Phase 3: 1–1.5 days.
- Phase 4: 2–3 days (including lock v2).
- Phase 5: 1.5–2 days.
- Phase 6: 1–2 days.
- Total: 9–13 focused days, plus review and CI time. If appliance decoupling
  or lock v2 is deferred, `--locked` must be restricted and the total drops by
  roughly 3–4 days, but the correctness gaps stay.

## 9. Acceptance criteria

1. `rpmx build --arch x86_64,aarch64 --runner bwrap --arch-jobs 2` on a host
   with a usable `F`-flag aarch64 handler builds both models, writes
   `packages/x86_64/` and `packages/aarch64/`, and exits 0. The two root locks
   name different arches.
2. `rpmx build --arch x86_64,aarch64 --runner auto` builds x86_64 with bwrap
   and aarch64 with bwrap + binfmt; no VM starts. On the same host,
   `--runner qemu` selects qemu-system, and fails with `RPMX-RUNNER-002` if
   `qemu-system-aarch64` is unavailable.
3. `rpmx build --arch aarch64` on a host without a usable handler fails before
   any step with an actionable `RPMX-RUNNER-002`.
4. A single failing arch still lets the other finish; exit code is that arch's
   failure; JSON contains every failure. A two-failure run reports both in arch
   order and exits with the first.
5. `--junit` contains all test cases from all arches, including an arch that
   failed after tests ran.
6. `rpmx build --arch x86_64,aarch64 --locked` succeeds against a schema-2 lock
   and fails with a precise arch-level difference when one arch drifts.
7. `rpmx build --arch x86_64,aarch64 --sbom-dir sboms` writes one SBOM per
   arch/cell and overwrites nothing.
8. `--arch x86_64,x86_64`, `--arch nope`, an arch outside the profile,
   `--arch x86_64,aarch64 --stage DIR`, `--root`, and `--test-root-lock` fail
   with actionable errors.
9. `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, and `cargo deny check licenses` pass.
   `cargo deny check advisories` follows the recorded policy from 0.4; no
   unreviewed ignore is added.
10. A pre-change SRPM still rebuilds (the embedded schema-1 lock is untouched).
11. Every build-related log line from a two-arch build carries the correct
    `[<arch>]` tag, including guest stdout/stderr; a single-arch build also
    carries its arch; no line is attributed to the wrong worker.
12. Each phase has a recorded scoped review (reviewers, findings, resolution)
    before the next phase starts.
