# Migration, adoption and how to falsify the design

## 1. The migration principle

Do not ask a distribution to switch. Ask it to **build one package twice** and compare.

The migration path is:

1. **Import** an existing spec into a recipe.
2. **Plan** the recipe and compare the plan to the spec's effective behaviour.
3. **Build** the recipe with the native SRPM path (`rpmx` only).
4. **Diff** the resulting RPMs (headers, payload, file metadata, dependencies, scriptlets).
5. **Adopt** package by package; keep the spec as the source of truth until the diff is clean.

This is how a new packaging format earns trust: by producing the same artifacts before asking anyone to change their workflow.

---

## 2. Importing a spec

`rpmx import-spec` is an assistant, not a magician. It should:

- parse the spec with RPM's own parser (via `rpmspec`) to get the effective tags, sections and file lists;
- map the mechanical parts;
- mark everything it cannot prove as a `TODO` with the original spec text and line number;
- refuse to silently translate unsafe constructs.

### Mapping table

| Spec construct | `package.yaml` equivalent | Confidence |
|---|---|---|
| `Name`, `Version`, `Epoch`, `Release` | `[package]` | High |
| `Summary`, `License`, `URL` | `[package]` | High |
| `SourceN`, `PatchN` | `[[source]]`, `[[patch]]` | High if URL + hash are available |
| `BuildRequires` | `build.packages` / `build.sysroot` | Medium: scope must be inferred |
| `Requires`, `Recommends`, etc. | `[output.dependencies]` | High for atoms; low for rich deps |
| `%description` | `package` / `outputs[]` | High |
| `%package` | `outputs[]` | Medium: inheritance rules need review |
| `%prep` + `%autosetup` | sources + patches | High if standard |
| `%build` with `%configure`/`%cmake`/`%meson` | `build.pipeline: - uses: <bundle>` | High |
| `%build` with custom shell | `build.pipeline` inline `argv`/`runs` | Medium |
| `%install` with `%make_install`/`%cmake_install` | bundle install step | High |
| `%install` with custom shell | inline pipeline step | Low: needs review |
| `%files` | `outputs[].files` | Medium: globs and attributes need care |
| `%pre`, `%post`, `%preun`, `%postun` | `[[output.action]]` or `shell` | Low: must be reviewed |
| `%trigger*`, `%filetrigger*` | `trigger` action | Low: ordering semantics need review |
| `%if` on distro macros | feature or profile | Low: intent must be recovered |
| `%global` / `%define` | none | None: rewrite by hand |
| `%changelog` | `[package] changelog` | High if parsed |
| `ExcludeArch`/`ExclusiveArch` | `[package] architectures` | High |
| `BuildArch: noarch` | `outputs[].arch: noarch` | High |
| `%config`, `%license`, `%doc`, `%caps`, `%ghost`, `%attr`, `%verify` | `[[output.file]]` | High |
| `%generate_buildrequires` | dependency generator / lock | Low: reimplement |

The importer should produce a **report**:

```
Imported 14 of 19 constructs mechanically.
Manual review required:
  - %build lines 42-67: custom shell with 3 sed commands and a patch application.
  - %post lines 120-138: systemd + tmpfiles + user creation; consider typed actions.
  - %if 0%{?fedora} lines 71-84: two dependency variants; decide whether this is a feature or a profile.
  - %global _foo lines 12-18: macro used in 6 places; no automatic equivalent.
```

### What not to import

- Macro definitions that compute values from the host environment.
- `%if` blocks whose condition depends on macros that are not in the spec.
- Scriptlets that modify files outside the package's ownership.
- `%files` globs that overlap between subpackages without an explicit resolution.
- `%generate_buildrequires` shell that queries the network.

These become `TODO` blocks or hard errors. The user must decide.

---

## 4. Adoption path

### Stage 0: reference packages

Pick 10–20 packages that cover the hard cases:

- a simple autotools package (`hello`),
- a CMake package with subpackages (`curl` or `zlib`),
- a Meson package with tests (`json-glib`),
- a Python package with vendored deps,
- a Go package with modules,
- a Rust package with `Cargo.lock`,
- a package with systemd units and sysusers,
- a package with alternatives,
- a package with a kernel module or DKMS,
- a package with architecture-specific sources,
- a noarch documentation package,
- a package with a complex patch series,
- a package with rich/weak dependencies.

For each, write the recipe, import the spec, build both, and diff.

**New undistros and new repositories are the best first adopters.** Wolfi shows that a new distribution can start with a clean package definition instead of converting a legacy archive. A new RPM-based undistro, a container-focused repository, or a vendor appliance distro can adopt `rpmx` without a migration backlog. Established distros should use the shadow-build path first, because their value is in the existing package set and policy, not in a flag day.

### Stage 1: plan-only CI

Run `rpmx lint` and `rpmx plan` on every PR. Do not build. This validates:

- schema,
- feature constraints,
- file selector overlaps,
- dependency syntax,
- profile overlays.

It gives reviewers a machine-readable plan without changing the build.

### Stage 2: shadow builds

Build the recipe in CI alongside the spec. Compare:

- NEVRA,
- provides/requires,
- file list and attributes,
- scriptlets,
- payload content (after normalizing timestamps),
- RPM header tags.

Any difference is either a bug in `rpmx` or an intentional difference. Record it.

### Stage 3: pilot SIG

Choose a distro SIG or a small repository. Make the recipe the source of truth there. Keep the spec export for downstream consumers. Measure:

- build success rate,
- review time,
- reproducibility rate,
- number of escape hatches,
- packager satisfaction.

### Stage 4: default for new packages

New packages use `package.yaml`. Existing specs are converted when they are next touched, not in a flag day.

### Stage 5: spec support ends

Only after the pilot has proven the format. Native SRPMs are rebuilt with `rpmx`; there is no spec export or `rpmbuild --rebuild` compatibility path.

---

## 5. Build-system and repository integration

`rpmx` should not try to replace mock/koji/OBS/Copr. It should integrate:

- **Build root**: `rpmx` asks the configured build service for a root that satisfies the dependency set, then records the root digest in the lock.
- **Repository**: `rpmx repo` generates metadata or delegates to `createrepo_c`.
- **Signing**: `rpmx` does not sign and holds no keys. RPMs and SRPMs are emitted unsigned; a repository or signing service signs them with `rpmsign` or an equivalent, outside the build sandbox.
- **SBOM/provenance**: `rpmx` emits sidecar files; the repository serves them next to the RPMs.
- **CI**: a GitHub/GitLab action that runs `lint`, `plan`, `build --locked --test --evidence`.

The native SRPM is the integration artifact. Build services that already ingest SRPMs can store it, but rebuilds require `rpmx`, not `rpmbuild`.

---

## 6. Roadmap

### M0: schema and linter (weeks)

- Define the YAML schema and generate a JSON Schema.
- Implement strict parsing, diagnostics, `rpmx fmt`, `rpmx lint`.
- Build a corpus of 50 recipes (ported from specs) and measure which fields are missing.

### M1: planner and native-writer spike (weeks)

- Implement feature resolution, dependency parsing, profile overlays.
- Expand bundles and the step library for `autotools`, `cmake`, `meson`, `make`, `cargo`, `go`, `python`, `npm`.
- Implement source locators and the source-assembly plan (archive, git, Go/vendor, generators).
- Spike the native SRPM writer: evaluate `rpm-rs`/`rpm` `PackageBuilder`, `librpmbuild` bindings, and a minimal header+payload writer.
- Implement `rpmx plan` with a textual and JSON output.
- No builds yet.

### M2: executor, source assembly and native SRPM (months)

- Implement fetch/CAS and the source-assembly sandbox.
- Implement the build sandbox and step runner.
- Implement the native SRPM writer: header, payload, compression, deterministic assembly.
- Implement the native binary writer or `librpm` binding: headers, file metadata, dependency generation, scriptlets and actions.
- Implement the file scanner and manifest.
- Build binary RPMs from the native SRPM with `rpmx`; no `rpmbuild` and no generated spec at any stage.
- Build `hello`, `zlib`, `curl` end to end, including `rpmx rebuild` from the SRPM.
- Compare against stock spec builds only as a migration check.
- No `--legacy-spec` output; native SRPMs are the only source artifact.

### M3: outputs, actions, tests (months)

- Implement multiple outputs, auto-split, file attributes.
- Implement typed actions and the profile emitters.
- Implement `rpmx test --install`.
- Implement SBOM generation and the provenance statement interface (unsigned in V1; signing is external).

### M4: lock, reproducibility, migration (months)

- Implement `rpmx lock`, `--locked`, `--reproduce`.
- Implement `import-spec` for migration; no spec export.
- Run the reference package corpus in shadow CI.
- Publish a compatibility report.

### M5: pilot (a release cycle)

- Pick a SIG/repository.
- Measure the success criteria.
- Decide whether to continue, change, or stop.

---

## 7. Success criteria

The design should be considered successful if, in a pilot:

1. **At least 80% of pilot packages** build with named steps alone, without inline `runs`.
2. **At least 90% of file/dependency/feature errors** are caught by `rpmx plan` before the build.
3. **At least 95% of `--locked` rebuilds** are bit-identical for packages with pinned toolchains.
4. **A distro policy change** (libdir, hardening flags, test requirement) requires no recipe edits.
5. **Reviewers** can answer "what files does this package own, what does it require, and what does it do at install time?" from `rpmx plan` without running the build.
6. **Packagers** prefer the recipe over the spec for new packages in a usability test.
7. **The generated RPMs** are accepted by existing RPM tooling without changes.

If (1) fails, the abstraction is wrong. If (2) fails, the schema is too weak. If (3) fails, the lock/sandbox is insufficient. If (4) fails, the profile boundary is wrong. If (6) fails, the ergonomics are wrong. If (7) fails, the backend is wrong.

---

## 8. Experiments that would falsify the design

### E1. Port 20 diverse specs

For each, record:

- recipe lines vs. spec lines,
- number of inline pipeline steps,
- number of `TODO`s from the importer,
- time to a clean build,
- diff against the spec build.

Falsifies: "the format is expressive enough for real packages."

### E2. Error injection

Deliberately introduce:

- a file claimed by two outputs,
- a staged file claimed by none,
- a feature that requires a disabled feature,
- a source hash mismatch,
- a profile overlay with a typo,
- a dependency with an invalid version,
- a `%config` outside `/etc`,
- a `noarch` output containing an ELF.

Verify that each is caught before the build with a useful message.

Falsifies: "the planner catches errors early."

### E3. Reproducibility

Build 50 packages twice in different directories and, where possible, different machines. Compare unsigned payloads and headers. Classify differences.

Falsifies: "the default build is reproducible."

### E4. Policy portability

Take 10 recipes and build them under two profiles with different `libdir`, hardening flags, vendor, dist tag and test policy. Verify that no recipe changes are needed.

Falsifies: "policy is separate from recipes."

### E5. Usability

Give 5 experienced RPM packagers and 5 packagers from Debian/Arch/Gentoo a simple package and a complex package. Measure time to first successful build, number of documentation lookups, and preference.

Falsifies: "the format is easier."

### E6. Generated-spec fidelity

For 20 packages, compare the `rpmx` RPM with the hand-written spec RPM:

- header tags,
- file list and attributes,
- requires/provides,
- scriptlets,
- payload file contents,
- signatures (separately).

Falsifies: "the backend preserves RPM semantics."

### E7. Performance

Plan 1,000 recipes in CI. Measure plan time and memory. If planning is slower than parsing specs, investigate before optimizing.

Falsifies: "the typed model is practical at distro scale."

---

## 9. Risks and mitigations

| Risk | Why it matters | Mitigation |
|---|---|---|
| **Over-abstraction** | Build systems are weird; a typed model may not express the long tail. | Inline pipeline steps; measure how often they are needed; add bundles and steps only for repeated patterns. |
| **Profile creep** | Profiles become a new macro language. | Limit v1 to set/append/remove/merge; no code; show a typed diff; reject unknown paths. |
| **Auto-split surprises** | A file lands in the wrong output. | `rpmx plan` shows the manifest; auto-split is policy-driven; ambiguous paths are errors. |
| **Sandbox portability** | bubblewrap/namespaces are not available everywhere. | Rootless container fallback; explicit `--no-sandbox` with provenance; CI uses a supported environment. |
| **Cache correctness** | A cache keyed on incomplete inputs produces wrong artifacts. | Only cache after hermetic execution; include plan/toolchain/source digests; allow `--rebuild`. |
| **Cross-compilation** | The hardest packaging problem. | Build/host/target scopes; sysroot in the profile; bundles and steps that know cross conventions; test with a real cross target. |
| **Native SRPM writer** | Writing source packages without a spec is non-trivial. | M1 spike; use `rpm-rs`/`librpmbuild` if viable; minimal writer fallback; validate with independent RPM tooling. |
| **Bootstrap** | Native SRPMs require `rpmx` to rebuild. | Static binary, signed bootstrap artifact, documented recovery path. |
| **Adoption** | Packagers are busy and skeptical. | Shadow builds, diff reports, incremental adoption, no flag day. |
| **Governance** | A schema controlled by one vendor will fail. | Open schema process; versioned changes; reference implementation with a compatibility policy; distro representatives. |
| **Build-root portability** | The tool must be buildable and runnable in minimal roots. | Static binary; vendored dependencies; a bootstrap profile; no Python runtime in the build root. |

---

## 10. Open questions

These are the questions I would want answered before freezing the schema:

1. **Is strict YAML the right surface, or is KDL better?** YAML is more familiar and has better tooling; KDL is more elegant and avoids YAML's typing baggage. The current recommendation is YAML with a strict profile.
2. **How much inline shell is acceptable?** `runs` is a first-class step form (see [`13-pipelines.md`](13-pipelines.md)); the question is whether an inline step needs a `reason`, and whether a complex case should live in a named step instead.
3. **Feature granularity.** Are features package-level, output-level, or both? Can a feature add an output? (V1 says no; is that too restrictive?)
4. **Profile power.** Is set/append/remove/merge enough for real distro deltas, or do profiles need typed "add output/action" operations?
5. **Dependency lock semantics.** Should the lock pin exact NEVRAs, a build-root digest, or both? What happens when a repository is not bit-reproducible?
6. **Build-root provisioning.** Does `rpmx` own the build root, or delegate to mock/koji? How is the root digest computed portably?
7. **Rich dependencies.** Is `any_of`/`all_of` enough, or do we need first-class `if`/`unless` for package-presence conditions?
8. **Weak dependency qualifiers.** How should `pre`/`post`/`meta` qualifiers interact with outputs and scriptlets?
9. **Scriptlet ordering.** Can typed actions be ordered deterministically without exposing RPM's transaction heuristics? What about triggers?
10. **Kernel modules / DKMS / akmods.** These have special build and install semantics; do they need first-class steps?
11. **Debuginfo.** Opt-in per package is decided and implemented as `uses: debuginfo` plus a declared output; there is no distro-wide flag. Still missing: a `debugedit`-equivalent path remap and `-debugsource`.
12. **Multi-version installs.** Do we need slots/subslots (Gentoo) or is `compat-` packaging enough?
13. **Noarch subpackages in arch builds.** How does the planner handle mixed-arch outputs and file conflicts?
14. **Source RPM semantics.** What exactly goes into the native SRPM, and can an independent RPM reader verify the header and payload?
15. **Schema evolution.** How do we add fields without breaking old recipes? Versioned schema plus a migration tool? Deprecation windows?
16. **Signing/provenance.** Which attestation format, and how does it integrate with repository metadata and transparency logs?
17. **Cross-distro profiles.** Can one recipe build on Fedora, RHEL, openSUSE and a derived distro without recipe edits? Where does the profile boundary actually hold?
18. **Performance at scale.** Is pure-function planning fast enough for 10k packages, or do we need incremental evaluation?

---

## 11. What would make me abandon this design

I would abandon or substantially revise it if:

- **Inline steps dominate.** If most real packages need inline `runs` or a one-off step, the step library is not earning its keep.
- **Profiles become unreadable.** If distro deltas require a complex overlay language, we have recreated macros with worse tooling.
- **The native writer cannot express the semantics.** If typed actions, file attributes or dependencies require falling back to a spec, the source-package design is wrong.
- **Packagers hate YAML.** If the strict profile is still error-prone or the ergonomics are worse than spec for experienced packagers, the format has failed regardless of its theoretical properties.
- **Reproducibility is not achievable in practice.** If `--locked` rebuilds routinely differ because of toolchain or filesystem nondeterminism, the lock is a false promise.
- **The ecosystem rejects the output.** If RPM consumers cannot handle the generated packages without changes, the "keep the RPM format" constraint has been violated.

The design is meant to be falsified, not defended.
