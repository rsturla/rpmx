# Bootstrap ladders

A bootstrap ladder rebuilds a distribution's own toolchain from a pinned seed.
It is a series of stages, not one build: each stage runs in its own root, writes
its own repository, and feeds the next stage. `rpmx bootstrap plan` validates
the manifest and shows the graph before anything resolves a package or starts a
build.

The engine owns the ordered stage mechanics. The distribution repository owns
the seed image, recipe inventory, generation manifest, promotion gates and
rollback policy.

## Why a separate manifest

A recipe describes one package and its source build. A profile describes one
root. A bootstrap ladder is neither: it is an ordered graph of package sets,
root pairs and output repositories, where the same recipe is rebuilt against a
different root in each stage. Keeping that graph in a dedicated document lets
recipes stay stage-agnostic and lets profiles stay ordinary profiles. A
`bootstrap:` section inside `package.yaml` would have to reference other
recipes, own several roots, and mix stage policy into source metadata; a
profile would have to own package selection and build order. Both couplings are
worse than one small orchestration manifest.

The manifest deliberately does not replace distro repository tooling. Seed
inventory, generation labels, promotion, rollback and verification gates remain
external; the manifest records the provenance it needs to drive the stages.

Per-stage files are a reasonable future extension if a ladder becomes large
enough that one review is unwieldy. The current single manifest is the
authoritative graph and can gain an `include:` or `stages: [stage0.yaml, ...]`
form without changing stage semantics.

## Plan a ladder
## Plan a ladder

```console
rpmx bootstrap plan bootstrap.yaml
```

The plan is pure: it discovers recipes, checks their dependencies, validates
the stage graph and prints the next command. It does not resolve roots or use
the network.

```text
seed   name=fedora-44 image=registry.fedoraproject.org/fedora:44 releasever=44 generation=2026.01
epoch  1704067200
stage0  profile=profiles/stage0.profile.yaml arch=x86_64 inputs=-
  root   bootstrap/roots/stage0/x86_64
  lock   bootstrap/locks/stage0/x86_64.lock.yaml
  repo   out/stage0/x86_64  id bootstrap-stage0
  seed   true
  tools  +bison +python3 +texinfo
  order  filesystem, binutils, glibc, gcc, make, bash, coreutils
stage1  profile=profiles/stage1.profile.yaml arch=x86_64 inputs=stage0
  ...
next: rpmx bootstrap build bootstrap.yaml
```

`--json` emits the same data as one document for CI. `--arch` selects one
architecture; without it, the plan expands every architecture in the manifest.

## Build a ladder

Plan first, then build. A single-architecture manifest needs no `--arch`; a
multi-architecture manifest requires one so each ladder is independent.

```console
rpmx bootstrap plan bootstrap.yaml
rpmx bootstrap build bootstrap.yaml --stage stage0 --arch x86_64
rpmx bootstrap build bootstrap.yaml --arch x86_64
```

`--stage NAME` builds one stage and requires its inputs' repositories, indexes
and locks to exist. Without it, every stage runs in manifest order. A
multi-architecture manifest refuses to guess:

```console
rpmx bootstrap build bootstrap.yaml --arch x86_64
```

Each stage has its own root and lock:

```text
bootstrap/roots/stage0/x86_64/root
bootstrap/roots/stage0/x86_64-sysroot/root      # cross stages only
bootstrap/locks/stage0/x86_64.lock.yaml
bootstrap/locks/stage0/x86_64.sysroot.lock.yaml # cross stages only
```

A cross stage resolves a host build root and a target sysroot. `extra-tools`
go into the build root; `extra-sysroot` go into the sysroot. Both are part of
the stage requirement, so they appear in that stage's lock and are absent from
later stages unless those stages declare them too. They never become runtime
dependencies of the built packages.

### Locked and offline builds

```console
rpmx bootstrap build bootstrap.yaml --arch x86_64 --locked
rpmx bootstrap build bootstrap.yaml --arch x86_64 --offline
```

`--locked` verifies each stage lock and root digest and fails with
`RPMX-BOOTSTRAP-006` when either is missing, stale or drifted. Run without
`--locked` once to create the locks, then commit them. `--offline` never
resolves or fetches; it reuses cached roots and fails with `RPMX-ROOT-016`
naming the stage and the online command when a root is absent.

`--refresh` re-resolves stage roots and cannot be combined with `--locked` or
`--offline`. Use it with `--force` when a non-empty root or stage output must be
replaced.

### Atomic stage output

A stage builds into a temporary sibling of its repository directory, writes
and indexes the RPMs, then renames the directory into place. A failed stage
does not leave a half-valid repository. A non-empty destination is refused
with `RPMX-BOOTSTRAP-008` unless `--force` is given:

```console
rpmx bootstrap build bootstrap.yaml --arch x86_64 --refresh --force
```

### Fixed point

A stage with `fixed-point` rebuilds the named recipes against a root that
already contains the stage output. Rebuilding a package usually keeps the same
NEVRA, so the engine filters the input repositories with `exclude` and adds the
stage output as the winning repository. This makes the stage converge on its
own toolchain instead of silently resolving the input's copy.

The first pass builds every stage recipe from the declared inputs. Each
following pass resolves a candidate root under
`roots/<stage>/<arch>-iter<i>/`, rebuilds
only the `fixed-point` recipes and re-indexes the stage output. It stops when
the SHA-512 of every produced RPM and the candidate root's closure and root
digests match the previous pass. A candidate that matches is promoted to
`roots/<stage>/<arch>` and `locks/<stage>/<arch>.lock.yaml`; a stage that
does not converge
within `max-iterations` fails with `RPMX-BOOTSTRAP-005`, naming the packages
whose bytes changed and the last two root digests.

`max-iterations` defaults to 3. The first pass is the ordinary stage build, so
an already stable toolchain normally needs one or two extra passes.

### Rebuild a stage package

A stage installs packages built from earlier stages, so rebuilding one of its
packages legitimately uses a different root than the package's embedded lock.
Use the bootstrap-specific command rather than relaxing `rpmx rebuild`:

```console
rpmx bootstrap rebuild stage0/probe.src.rpm \
  --root-lock bootstrap/locks/stage1/x86_64.lock.yaml \
  --root bootstrap/roots/stage1/x86_64/root \
  --output out/stage1/x86_64 \
  --srpm-out out/stage1/src
```

The command replaces the embedded root lock with `--root-lock`, builds the
binary RPMs, and re-emits a new SRPM under `--srpm-out`. The new SRPM embeds the
stage lock and records the old closure/root digests under `replaced` in
`package.lock.json`. The input SRPM is never modified: choosing a different
`--srpm-out` directory is required, and the same directory is refused.

Plain `rpmx rebuild --root-lock <mismatched>` remains strict and still fails
with `RPMX-ROOT-016`. Pick `rpmx bootstrap rebuild` only when a stage lock is
the intentional replacement.

### Failure recovery

A failed stage names the stage, architecture and underlying diagnostic. Fix
the reported recipe or root problem, then rerun that stage. Its input
repositories and locks are already valid:

```console
rpmx bootstrap build bootstrap.yaml --stage stage1 --arch x86_64 --locked
```

If a root is missing and no lock exists, run without `--locked` to resolve it.
If a recipe or stage extra changed, the old lock is stale; run without
`--locked` to update it intentionally, or use `--refresh` with `--force`.

## Manifest

```yaml
document: rpmx-bootstrap
version: 1
name: eternal
archs: [x86_64]
recipe-root: recipes               # default "."
roots: bootstrap/roots             # default
locks: bootstrap/locks             # default
source-date-epoch: 1704067200      # deterministic repo metadata
max-iterations: 3                  # fixed-point passes per stage (default 3)

seed:                              # optional provenance
  name: fedora-44
  image: registry.fedoraproject.org/fedora:44
  releasever: "44"
  generation: "2026.01"

stages:
  - name: stage0
    profile: profiles/stage0.profile.yaml
    packages: [filesystem, binutils, glibc, gcc, make, bash, coreutils]
    output: out/stage0
    seed: true
    extra-tools: [bison, python3, texinfo]
    extra-sysroot: []

  - name: stage1
    profile: profiles/selfhosted.profile.yaml
    packages: all
    output: out
    fixed-point: [binutils, glibc, gcc]
    replaces:                      # only when names differ
      eternal-coreutils: coreutils
      eternal-util-linux: [util-linux, util-linux-core]
```

`schema: 1` is accepted as an alias for `version: 1`. Unknown fields are
errors. Paths are resolved relative to the manifest; `..` is allowed, so a
manifest in `.bootstrap/` can use `recipe-root: ../packages` and
`profile: ../profiles/bootstrap.profile.yaml`.

### Stage fields

- `profile` is a normal rpmx profile. A profile with a `cross` block makes the
  stage a cross stage; `cross.bootstrap: true` allows a native bootstrap whose
  build root and sysroot share an architecture.
- `packages` lists package names, `all`, or both. Recipes are discovered by
  scanning `recipe-root` for `package.yaml` and reading `package.name`.
- `recipes` selects explicit paths or globs. When both `recipes` and `packages`
  are present, `packages` filters the selected set.
- `output` is the stage repository root. The build writes `<output>/<arch>`.
- `inputs` names earlier stages; it defaults to the immediately previous stage.
  Stage 0 has no inputs. Forward references are rejected.
- `seed: true` marks the seed stage. The marker is provenance and policy, not a
  second dependency mechanism.
- `extra-tools` adds build-root packages to this stage only.
- `extra-sysroot` adds target packages to this stage's sysroot only.
- `order` adds explicit package edges, for example
  `order: [[glibc, gcc], [gcc, glibc]]`.
- `fixed-point` lists packages rebuilt until the output and root digests stop
  changing. A dependency cycle is accepted only when every member is listed
  here. The input repositories exclude the stage output's package names and
  the stage output repo wins; `max-iterations` bounds the passes.
- `replaces` maps an Eternal output package name to the seed package name or
  names it replaces. The names do not have to match. A non-seed stage adds the
  seed names to the profile repositories' `exclude` list, and a fixed-point
  pass adds them to the input repositories as well. The output RPM must still
  declare the seed name in its `provides` when other packages depend on that
  name; `replaces` only controls which provider the resolver may see.
- `source-date-epoch` overrides the manifest timestamp for this stage.

### Seed-stage policy

A seed stage, and any stage whose profile uses `cross`, must name an explicit
`packages` or `recipes` list. It may not use `packages: all`: a new recipe must
not enter the bootstrap closure by accident. `all` is allowed only for the
final self-hosted stage.

### Source date epoch

`repo::index` needs a deterministic timestamp. Precedence is stage, then
manifest, then the selected recipes. If neither the stage nor the manifest sets
a value and the recipes disagree, the plan fails and lists both values instead
of choosing wall-clock time.

### Seed metadata

The top-level `seed:` block is provenance printed in the plan and copied into
stage locks. It is not a resolution mechanism; the stage profile and its root
lock remain the source of truth. If `seed.image` or `seed.releasever` disagrees
with the stage 0 profile, the plan fails early.

## Common errors

| Diagnostic | Fix |
|---|---|
| `RPMX-BOOTSTRAP-001` | Fix the manifest field named in the message. |
| `RPMX-BOOTSTRAP-002` | Use a safe stage name and an earlier stage as `inputs`; give every stage a unique output. |
| `RPMX-BOOTSTRAP-003` | Name explicit packages/recipes on a seed or cross stage; fix the missing or duplicate recipe. |
| `RPMX-BOOTSTRAP-004` | Add an `order` edge, or list every cycle member in `fixed-point`. |
| `RPMX-BOOTSTRAP-005` | The fixed point did not converge: inspect the changed packages, make the toolchain reproducible, or raise `max-iterations`. |
| `RPMX-BOOTSTRAP-006` | The stage lock or root digest is stale or drifted. Rebuild without `--locked` once, then commit the lock. |
| `RPMX-BOOTSTRAP-007` | Build the stage's inputs first; the message prints the exact command. |
| `RPMX-BOOTSTRAP-008` | The stage output is non-empty; pass `--force` to replace it atomically. |
| `RPMX-BOOTSTRAP-009` | Use an architecture listed by the manifest. |
