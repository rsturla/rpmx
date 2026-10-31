# A better RPM build definition — design study

This repository is a design study, not an implementation.

The question: **if we could replace the RPM spec file with a build definition designed for the 2020s, while still producing ordinary RPM packages, what should it look like?**

The short answer:

> Keep RPM's *package format* and its best semantics. Replace the *authoring format* with a typed, declarative recipe; move distribution policy out of the recipe; make the build a deterministic plan over locked inputs; make file ownership total and checkable; and treat tests, SBOMs and provenance as first-class outputs.

Working name for the tool: **`rpmx`**. Working name for the recipe: **`package.yaml`**. Both are placeholders; the design does not depend on the names.

> **Surface syntax note.** The first draft used TOML. It is typed and easy to parse, but the `curl` example showed that TOML's array-of-tables and inline-table syntax fight the package tree. The recommendation is now **strict YAML 1.2** for the human-facing recipe, with JSON for the generated lock. YAML and JSON are first-class codecs over one canonical model; TOML and KDL can be supported as import/export converters without becoming separate authoring surfaces. See [`docs/05-format-choice.md`](05-format-choice.md) and [`examples/curl/package.yaml`](../examples/curl/package.yaml). The TOML example remains only as a comparison.

## What is here

| Document | Contents |
|---|---|
| [`docs/01-landscape.md`](01-landscape.md) | Survey of ~30 packaging/build-definition systems, including Wolfi/melange/apko, Chisel, OSBuild, mkosi, bootc and image-first tools, with a comparison matrix and per-system "better/worse than rpmbuild" notes. |
| [`docs/02-rpm-baseline.md`](02-rpm-baseline.md) | What RPM gets right, what the spec language gets wrong, and which RPM 4.20/6 features are partial fixes. |
| [`docs/03-design.md`](03-design.md) | The proposed design: data model, semantics, phases, features, outputs, actions, tests, profiles, lockfile, CLI, errors, and implementation strategy. |
| [`docs/04-migration.md`](04-migration.md) | Import/export, adoption path, roadmap, success criteria, and open questions. |
| [`docs/05-format-choice.md`](05-format-choice.md) | Why TOML is the wrong surface, and why strict YAML 1.2 is the recommendation. |
| [`docs/06-build-inputs.md`](06-build-inputs.md) | Replacing the lookaside cache with content-addressed sources, locators, a local CAS, OCI artifacts, SRPMs and source assembly. |
| [`docs/07-srpm-first.md`](07-srpm-first.md) | The SRPM-first model: every build produces an SRPM, and every build can be reproduced from one. |
| [`docs/08-threat-model.md`](08-threat-model.md) | Threat model for proxies, SRPMs and build inputs: what content addressing protects, and what it does not. |
| [`docs/09-sandboxing.md`](09-sandboxing.md) | Build sandbox contract, backend comparison (bubblewrap, nsjail, Podman, gVisor, microVMs), and the recommended profiles. |
| [`docs/10-container-distribution.md`](10-container-distribution.md) | Distributing `rpmx` and builder images as signed OCI containers, including `/dev/kvm` and socket policy. |
| [`docs/11-matrices.md`](11-matrices.md) | Build matrices: typed axes, per-cell SRPMs and dependencies, naming, exclusions, and how this generalizes melange's `data`/`range`. |
| [`docs/12-templates.md`](12-templates.md) | The closed template scope: package identity, profile, matrix cell and user variables, expansion order, escaping and validation. |
| [`docs/13-pipelines.md`](13-pipelines.md) | The build pipeline contract: named steps and bundles, typed inputs, `needs`, inline `argv`/`runs`, lock entries and plan output. |
| [`docs/14-build-root.md`](14-build-root.md) | Build-root resolution and assembly: dnf in a rootless container, lock shape, offline assembly and open questions. |
| [`examples/README.md`](../examples/README.md) | Index of the worked examples. |
| [`examples/curl/package.yaml`](../examples/curl/package.yaml) | The same recipe in the proposed human-facing YAML syntax. |
| [`examples/curl/package.toml`](../examples/curl/package.toml) | The provisional TOML version, kept for comparison only. |
| [`examples/curl/curl.spec`](../examples/curl/curl.spec) | The same package as a conventional spec, for comparison. Not a complete production spec. |
| [`examples/curl/plan.md`](../examples/curl/plan.md) | A worked elaboration from recipe to build plan and output manifest. |
| [`examples/hello-go/package.yaml`](../examples/hello-go/package.yaml) | A Go package pulled from a module proxy, vendored, and rebuilt from an SRPM. |
| [`examples/go-bump/package.yaml`](../examples/go-bump/package.yaml) | CVE remediation with targeted Go module bumps via omnibump in source assembly. |
| [`examples/libfoo/package.yaml`](../examples/libfoo/package.yaml) | Deterministic generated source in the source-assembly stage. |
| [`examples/python-matrix/package.yaml`](../examples/python-matrix/package.yaml) | A Python library built as a matrix across Python 3.10–3.12, with per-cell SRPMs and outputs. |
| [`examples/c-hello/package.yaml`](../examples/c-hello/package.yaml) | A tested C fixture built with the `make` step; used to verify symlinks and the reusable pipeline. |
| [`examples/zstd/package.yaml`](../examples/zstd/package.yaml) | A tested real package: zstd as `libzstd`, `libzstd-devel` and `zstd`, rebuilt byte-identically from a locked root. |
| [`examples/libgreet/package.yaml`](../examples/libgreet/package.yaml) | A tested shared library with a devel subpackage, pkgconfig metadata and an installed test that compiles a consumer. |
| [`examples/zlib/package.yaml`](../examples/zlib/package.yaml) | A tested autotools library with a devel split. |
| [`examples/xz/package.yaml`](../examples/xz/package.yaml) | A tested autotools package with `xz-libs`, `xz-devel`, `xz` and versioned symbol provides. |
| [`examples/lua/package.yaml`](../examples/lua/package.yaml) | A tested Makefile-based interpreter package. |
| [`examples/jq/package.yaml`](../examples/jq/package.yaml) | A tested autotools package with a system library dependency and a JSON CLI test. |
| [`examples/pyhello/package.yaml`](../examples/pyhello/package.yaml) | A tested pure-Python noarch wheel package with separately resolved test dependencies. |
| [`examples/hello-daemon/package.yaml`](../examples/hello-daemon/package.yaml) | A tested systemd service with sysusers/tmpfiles, scriptlets and `%config(noreplace)` upgrade semantics. |
| [`examples/python-matrix/plan.md`](../examples/python-matrix/plan.md) | Worked plan for the matrix example: expansion, per-cell dependencies, lock entries and naming. |
| [`spikes/README.md`](../spikes/README.md) | Runnable prototypes that test the riskiest claims: native SRPM, native binary RPM, and the bwrap sandbox. |

## The eleven findings that shaped the design

1. **The spec file is five languages in a trench coat**: metadata tags, a parse-time macro preprocessor, `%if` conditionals, shell, and a file-glob manifest. Most spec complexity comes from their interaction, not from packaging itself.
2. **The strongest systems separate "what" from "how" from "where"**. Debian separates `control` / `rules` / `*.install`; Yocto separates recipes / classes / layers / distro config; Nix separates package functions / overlays / system configuration. RPM puts all of it in one file.
3. **Typed is better than clever.** Gentoo's `REQUIRED_USE`, Spack's specs, Conan's `package_id`, and Debian's build profiles all beat ad-hoc `%if` expressions because constraints are data that a solver can check.
4. **Hermeticity is a feature, not a compiler flag.** Nix, Guix, Bazel and (partly) Yocto show that "only declared inputs exist" is what makes caching, reproducibility and auditability possible. RPM builds are host-contaminated by default.
5. **A lockfile is the missing contract.** Nix has derivations, Spack has `spack.lock`, Conan has lockfiles, Cargo/Go/npm have lockfiles. RPM has a source RPM and a pile of macros. The recipe should be intent; the lock should be the exact resolved inputs.
6. **File ownership should be total, not a glob.** RPM's `%files` is powerful but can silently under- or over-capture. CRUX's `.footprint` and Debian's `*.install` point at the fix: compute an explicit manifest, then assert every staged file is claimed exactly once.
7. **Subpackages are outputs, not a second parse.** `%package` adds a second preamble section and duplicates metadata. Arch, Alpine, Void, Yocto and Flatpak all model multiple outputs more directly.
8. **Tests and provenance belong in the definition.** Homebrew's `test do`, Debian's autopkgtest, Spack's `spack test`, and Yocto's QA/SBOM are ahead of `%check`, which is optional and often disabled.
9. **Escape hatches are necessary; unconstrained escape hatches are the problem.** Every system has shell or a general-purpose language. The design should keep it visible: inline pipeline steps, declared tools, shown expanded in the plan.
10. **RPM's runtime semantics are worth keeping.** Rich/weak dependencies, file triggers, `%config`, `%license`, `%caps`, `%ghost`, debuginfo, subpackages, signatures and the RPM database are genuinely good. The problem is the authoring layer, not the package format.
11. **The modern undistro model validates the separation thesis.** Wolfi builds APK packages with melange, then composes images with apko. apko deliberately has no `RUN` equivalent: the image is a declarative list of packages plus configuration, which is what makes it bit-reproducible and SBOM-complete. This is the cleanest recent evidence that package build and image composition should be separate layers, and that "no arbitrary build steps in the composition layer" is a feature. It also shows that per-version output families belong in the recipe as data, not in `%if` — Wolfi expands Python packages 3.10–3.13 from one file with `data`/`range`. The caveat is that melange's YAML `runs:` pipelines can still become shell soup if a project lets them, and its matrix stops at subpackages.

## The design in one page

- **Recipe**: one strict, schema-versioned `package.yaml`. Unknown fields are errors. No macros, no `%if`, no host introspection during parsing.
- **Build**: an ordered pipeline of named steps (`uses`) plus inline `argv`/`runs`, as in melange. Bundles (`cmake`, `meson`, `python`, ...) compose common sequences; `rpmx plan` prints the expanded commands. Network is off after fetch; only declared tools and dependencies are mounted.
- **Dependencies**: separate `tools` (build machine), `host` (target sysroot), `target` (runtime for cross), `tests`, and `run`. Typed atoms, `any_of` groups, and `when: [feature]`, never raw `%if`.
- **Features**: data, not code. A `features.ssl` block can add dependencies, pipeline inputs, patches and files; constraints (`requires`, `conflicts`) are validated before building.
- **Outputs**: first-class `outputs:` lists. Each output declares metadata, dependency sets and file selectors. The planner computes a manifest, rejects duplicate or unowned files, and assigns standard splits (`libs`, `devel`, `doc`, `locale`) by policy. Debuginfo is opt-in, not automatic.
- **Actions**: typed system integration (`systemd`, `sysusers`, `tmpfiles`, `ldconfig`, `alternatives`, `desktop-database`, triggers) instead of hand-written scriptlets. Raw shell actions exist but are explicit, justified and linted.
- **Tests**: build tests and installed-system tests are declared, with their own dependencies. `rpmx test --install` installs the produced RPMs into a disposable root and verifies the manifest, actions and dependency closure.
- **Policy**: distribution rules live in versioned profiles, not in the recipe. Profiles are typed overlays (`set` / `append` / `remove`), and `rpmx plan --profile` shows the diff.
- **Sources**: content-addressed inputs (digest + locators), a local CAS, standard transports (HTTPS, OCI, LFS, file), and native SRPMs. No mandatory distro-specific lookaside.
- **Source assembly**: a deterministic pre-SRPM stage for vendoring, Git archives, generated code and license collection. It runs in its own sandbox, has no network after fetch, and records the snapshot digest and generator versions in the manifest.
- **Lock**: `package.lock.json` records source hashes, resolved features, profile digest, exact dependency NEVRAs, the rootfs digest, every step digest and output manifest digests. `--locked` refuses drift in any of them.
- **Output**: ordinary binary `.rpm` files plus a **native `.src.rpm`** containing the recipe, lock, sources, patches and manifest. Native SRPMs have no spec and are rebuilt with `rpmx`; there is no `rpmbuild --rebuild` compatibility mode.
- **Matrix**: typed axes expand one recipe into build cells, each with its own dependencies, build root, outputs and SRPM. Per-version Python families are the canonical case; melange's `data`/`range` is the prior art for the output side.
- **Templates**: a closed variable scope for package identity, profiles, matrix cells and user vars. No expressions, no conditionals, fixed-point expansion with cycle detection.
- **Build root**: `rpmx` resolves packages from declared repositories, records exact NEVRAs and pins a rootfs digest; builder images are a distribution form of the same root.

## Decided

These were open questions and are now settled; the documents above implement them.

| Decision | Choice |
|---|---|
| Binary RPM assembly | Native writer required before any release. No `rpmbuild`, no generated spec, not even temporarily. |
| Build root | `rpmx` resolves and pins NEVRAs plus a root digest. No mock/koji delegation in V1. |
| Proxy protocol | Content-addressed HTTPS `GET /v1/blobs/sha512/<hex>`; OCI and lookaside are backends. |
| Signing | Optional and caller-supplied: `rpmx keygen` creates a key where you ask, and `rpmx sign` or `--signing-key` signs explicitly. Builds are unsigned by default; `rpmx` never stores or discovers keys. |
| Provenance | Unsigned statement and signing interface specified in V1; signed attestations later. |
| Debuginfo | Opt-in only; never automatic. |
| Cross-compilation | Out of scope for V1; the dependency scopes remain in the model. |
| Matrix artifacts | One SRPM per cell; sources deduplicate in the CAS. |
| Lock contents | NEVRAs, root digest and step digests. |
| Schema evolution | Major-versioned schema, unknown fields are errors, `rpmx migrate` at a new major. |

## Scope and assumptions

- General-purpose Linux distributions producing RPMs.
- Source builds and binary-only packages; not an image composer (that is Yocto, Buildroot, kiwi, apko or bootc territory).
- Cross-compilation matters, but native builds are the default.
- No compatibility with existing spec files. A converter is desirable; semantic equivalence is not promised.
- The design assumes a sandbox (bubblewrap, rootless container, or equivalent) is available. It degrades explicitly, not silently.
- "Reproducible" means *bit-identical given the same lock, profile and toolchain*, verified by rebuilding. It does not mean "reproducible across arbitrary toolchains".

## Status

Partially implemented. The Rust workspace in `crates/` provides:

- `rpmx-core`: strict recipe parsing, matrix and template planning, local step resolution, source snapshot, stage splitting, lock and manifest generation, and a bwrap step runner.
- `rpmx-rpm`: native SRPM and binary RPM writers plus SRPM extraction, using a patched `rpm` crate (`vendor/rpm`).
- `rpmx` CLI: `lint`, `plan`, `fetch`, `source`, `root resolve|assemble|digest|export|import`, `srpm`, `build` (full pipeline or `--stage`), `rebuild`, `keygen`, `sign`, `repo` and `cache gc`.

The SRPM path is verified against stock RPM 6: it reads the package, accepts the header and payload digests, and extracts the payload. The binary writer produces installable RPMs and records `SOURCERPM`; a staged tree is split across outputs with totality checks. `rpmx rebuild` extracts an SRPM, runs its build steps in `bwrap`, and writes binary RPMs. Built-in steps — including `go/bump`, `go/build`, CMake, Meson, Autotools, Make and Python — are embedded YAML files, resolved by the same code as local `pipelines/` steps. The SRPM embeds those YAML files too, and `rebuild` verifies their digests against the lock and uses the SRPM copies, so a later `rpmx` cannot silently change a locked build. A Go fixture was built end to end, installed, run and erased in Fedora 44.

`rpmx fetch` populates a content-addressed cache from `https`, `file` and `go-module` locators, trying the content-addressed proxy first and verifying SHA-512 before publishing to the CAS. `--offline` serves only from the cache. `rpmx source --root-lock … --root …` then unpacks the fetched sources, applies patches, runs `source-assembly.pipeline` in the locked root, records `source-root.lock.json`, and writes a deterministic `RPMX-SOURCE.tar.gz`; `--unsafe-host-root` is the explicit escape hatch. `rpmx srpm` embeds it and `rpmx rebuild` unpacks it before the build. The full chain was run end to end with fixtures.

`rpmx root resolve --profile examples/fedora-44.profile.yaml` runs dnf5 in a rootless `podman` container with an empty installroot and only the profile's repositories, downloads the resolved closure into the CAS, and writes a `root.lock.yaml` with exact NEVRAs, SHA-512 payload digests, repository identity, resolve options and a `closure_digest`. Two runs produce byte-identical locks. `rpmx root assemble root.lock.yaml --root ./root` installs that closure offline from the CAS into an empty root, verifies the installed NEVRA set against the lock, and records a `root_digest` over a canonical listing of the tree. Two independent assemblies produce the same digest. `rpmx root export` emits a libpkgmanifest `rpm-package-manifest` 0.2.3 document and `rpmx root import` reads one back. `rpmx srpm --root-lock root.assembled.yaml` embeds the assembled root lock in the SRPM, together with the expanded build requirements. `rpmx build package.yaml --root-lock root.assembled.yaml --root ./root --output ./out` runs the whole chain in one invocation: source snapshot, native SRPM in `out/src`, then binary RPMs in `out`, with `--run-tests`, `--signing-key` and `--test-root-lock` applying to the same run. `rpmx rebuild package.src.rpm --root ./root` uses that embedded lock by default (an explicit `--root-lock` must match it), verifies `closure_digest` and `root_digest`, then binds the root read-only at `/` with bubblewrap and runs the build steps with no network and writable `/tmp/build` and `/tmp/stage`. `rpmx source --root-lock … --root …` runs source assembly in a locked root too. Patches declared in the recipe are applied before source assembly. `--run-tests` runs build-stage tests in the build sandbox and installed-stage tests in a fresh test root: `build.dependencies.tests` are resolved from the same repositories, assembled offline, and the built RPMs are installed over it with real dependency checking (no `--nodeps`). The resolved test root is written next to the RPMs as `<name>-<version>-<release>.test-root.lock.json`; `--test-root-lock FILE` reuses a pinned one and rejects drift from `build.dependencies.tests`. The profile carries the artifact policy: `debuginfo: true` injects `-g` via `CFLAGS`/`CXXFLAGS`, extracts debug sections and emits a `<name>-debuginfo` package; `sbom: true` embeds an SPDX 2.3 document in every RPM at `/usr/share/sbom/<name>-<version>-<release>.spdx.json` and writes a build-level SPDX document next to the RPMs covering the binary packages, the root closure, the source files and the recipe. The per-package documents link the package to its source RPM, upstream source files and build configuration. Both are recorded in the root lock and the SRPM, so a rebuild reproduces them. `--sbom FILE` only overrides the build-level path. `-debugsource` is not generated. `--root-image REF` remains the image-based alternative.

File metadata in a recipe is explicit: `files.directories` adds empty directories with mode/user/group, and `files.attributes` sets per-path mode, user, group and RPM capabilities (`cap_net_raw=ep`). Modes are octal strings; capabilities are validated by RPM. A recipe may also declare `package.cpe` (`vendor`, `product`, optional `part`/`edition`/`language` fields); `rpmx` renders CPE 2.3 strings and adds a `cpe23Type` security reference to both the per-package and build-level SPDX documents. Source acquisition supports `git` locators pinned by full commit; the tree is packed into a deterministic archive before it enters the CAS. `rpmx repo --rpms DIR --output REPO` writes primary/filelists/other metadata and can sign `repomd.xml.asc` with the same key used for packages.

Release identity is distribution policy plus a build input. A profile may set `dist-tag: .fc44`, and every command that plans a recipe (`lint`, `plan`, `source`, `srpm`, `build`, `rebuild`) accepts `--dist-tag` as an override. The build system supplies `--build-release N`, rendered as `+buildN` after the dist tag, matching Fedora's `%buildrelease` convention. The planner appends both after the matrix suffix, records them and the final release in `package.lock.json`, and `rpmx rebuild` reuses the locked values or fails with `RPMX-SRPM-012` on a conflicting flag. The example Fedora profiles set `.fc44`.

Package signing is optional: `rpmx keygen` writes an ASCII-armored OpenPGP key pair, and `rpmx sign --signing-key KEY` signs RPMs in place. `rpmx srpm`, `rpmx build` and `rpmx rebuild` accept the same `--signing-key` plus `--signing-passphrase-env`; a signed build records the key fingerprint in the build-level SPDX annotations, and the BOM checksums cover the signed files. Sign during the build when you need that consistency: a later `rpmx sign` changes the RPM but cannot update a BOM it does not know about. Keys stay in the caller's process and are never used inside the build sandbox. The dnf harness can run with `gpgcheck=1` when given a public key. `rpmx cache gc --lock LOCK --recipe RECIPE` removes unreferenced blobs while retaining recent ones, so a concurrent fetch is not deleted.

Acceptance harnesses live in `scripts/`: `dnf-corpus-test.sh` (install, upgrade and remove, optionally with `gpgcheck=1`, using `rpmx repo` metadata), `replacement-dnf-test.sh` (`Provides`/`Obsoletes`/`Conflicts` through `dnf swap`) and `systemd-dnf-test.sh` (sysusers, tmpfiles, scriptlet arguments, `%config(noreplace)` upgrade and erase).

Still missing: `-debugsource`, generic actions/file triggers, scriptlet dependency generators, weak and rich dependencies, repository snapshot pinning, module streams, SELinux labels, arbitrary xattrs beyond file capabilities, `%ghost`, multilib, signed provenance attestations, seccomp filters and cgroup memory/CPU limits, and a network-enforcing acquisition proxy for root resolution. The language pipelines are partial: Cargo, npm, Go and Python have build/vendor steps, but source materialisation for every ecosystem is not complete. CPE metadata is emitted into SBOMs but not into the RPM header, which has no standard CPE tag in the current writer. The host-root bwrap backend remains only behind `--unsafe-host-root` for source assembly.

The `curl` recipe is a worked example, not a tested build. The `examples/go-bump` and `examples/hello-go` recipes do plan and produce SRPMs from a local source snapshot. `examples/fedora-44.profile.yaml` is the minimal root fixture; `examples/fedora-44-build.profile.yaml` adds `gcc`, `make`, `patch` and `python3`. `examples/c-hello` is built end to end from that locked root with the reusable `make` step, and `examples/zstd` builds a real upstream project into `libzstd`, `libzstd-devel` and `zstd`. `examples/hello-daemon` proves the systemd and config paths: sysusers/tmpfiles files, unit installation, scriptlet arguments, `%config(noreplace)` on upgrade and `%config` save on erase. Two zstd rebuilds produced byte-identical RPMs once the static archive was built deterministically. The built-in `make`, `cmake`, `meson` and `autotools` steps pass `$JOBS` through; `make` also accepts `args` and `install_args`.

## Primary sources

The full reference list is at the end of [`docs/01-landscape.md`](01-landscape.md). The most important ones:

- RPM spec format and declarative builds: <https://rpm.org/docs/latest/manual/spec.html>, <https://rpm.org/docs/latest/manual/buildsystem.html>
- RPM 6 timeline: <https://rpm.org/timeline>
- Debian Policy and debhelper: <https://www.debian.org/doc/debian-policy/>, <https://manpages.debian.org/unstable/debhelper/dh.1.en.html>
- Arch PKGBUILD: <https://pacman.archlinux.page/PKGBUILD.5.html>
- Gentoo devmanual: <https://devmanual.gentoo.org/>
- Nixpkgs stdenv: <https://nixos.org/manual/nixpkgs/stable/#chap-stdenv>
- Spack packaging: <https://spack.readthedocs.io/en/latest/packaging_guide_creation.html>
- Yocto recipe manual: <https://docs.yoctoproject.org/dev/dev-manual/new-recipe.html>
- Flatpak manifests: <https://docs.flatpak.org/en/latest/manifests.html>
- Bazel hermeticity: <https://bazel.build/basics/hermeticity>
- Wolfi: <https://github.com/wolfi-dev/os>
- melange build files and pipelines: <https://github.com/chainguard-dev/melange>, <https://github.com/chainguard-dev/melange/blob/main/docs/BUILD-FILE.md>
- apko file format: <https://github.com/chainguard-dev/apko/blob/main/docs/apko_file.md>
