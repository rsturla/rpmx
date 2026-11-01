# RPM and the spec file: what to keep, what to replace

## What `rpmbuild` actually is

`rpmbuild` is a pipeline:

1. Read a spec file.
2. Evaluate conditionals and expand macros.
3. Parse the resulting text into a package header plus scriptlets.
4. Execute `%prep`, `%generate_buildrequires`, `%conf`, `%build`, `%install`, `%check`.
5. Walk the build root, apply `%files` patterns and file attributes, run dependency generators, and write one or more binary RPMs plus an SRPM.

That pipeline is sound. The problem is the *input language* and the fact that steps 1–3 are entangled with the host environment.

The RPM manual describes the model precisely:

- "Each line in the spec is macro-expanded before further processing."
- "Conditionals are evaluated first and then macros expanded. Only then are the sections parsed by the rules of the section types."
- Comments are macro-expanded too, unless escaped or written with `%dnl`.
- `BuildArch` "causes the spec parsing to recurse from the start, causing any macros before that line to be expanded twice."

Those four sentences explain most of the format's surprises. A spec is not a document that can be read; it is a program whose output is a package definition.

Sources: [RPM spec format](https://rpm.org/docs/latest/manual/spec.html), [declarative builds](https://rpm.org/docs/latest/manual/buildsystem.html), [rpmbuild(8)](https://man7.org/linux/man-pages/man8/rpmbuild.8.html).

---

## The five languages in one file

| Layer | What it is | Where it lives |
|---|---|---|
| Metadata | Name, version, license, summary, dependencies | Preamble tags |
| Preprocessor | `%define`, `%global`, `%{...}`, `%{lua:...}`, `%include` | Everywhere, including comments |
| Control flow | `%if`, `%ifarch`, `%ifos`, `%bcond`, `%elif`, `%else` | Preamble and body |
| Build program | Shell for `%prep`, `%conf`, `%build`, `%install`, `%check` | Body sections |
| Manifest | `%files` globs plus `%attr`, `%config`, `%doc`, `%license`, `%caps`, `%ghost`, `%verify` | `%files` sections |

Plus runtime shell (`%pre`, `%post`, `%preun`, `%postun`, `%trigger*`, `%filetrigger*`) and a human-readable `%changelog` block.

Each layer is individually reasonable. The cost is that they are not separable:

- You cannot validate the metadata without evaluating the preprocessor.
- You cannot know which dependencies exist without evaluating the conditionals.
- You cannot know which files are packaged without running the build and expanding globs.
- You cannot know which scriptlets exist without expanding macros in the script bodies.
- You cannot reproduce the parse without reproducing the entire macro environment.

This is the root cause of most spec pain. It is not that RPM lacks features. It is that the authoring format has no phase separation.

---

## Concrete pain points

### 1. Distro policy lives inside package recipes

A typical spec contains things like:

```spec
%if 0%{?fedora} >= 40
BuildRequires: openssl-devel >= 3.2
%else
BuildRequires: openssl-devel >= 3.0
%endif

%if 0%{?rhel}
%global _hardened_build 1
%endif
```

This makes the package recipe a function of the build host's macro files. Reviewing a change means knowing which macros the target distro defines. The same source can produce different dependency graphs on different machines, and the spec itself does not say which environment it assumes.

Modern RPM has `%bcond` and conditional builds (`--with`/`--without`), which are better, but they are still untyped and still evaluated at parse time.

### 2. Macros are expanded everywhere, including where you do not expect it

Because comments are expanded, a comment can fail the build. Because `BuildArch` re-parses, a `%global` can run twice. Because macros are expanded before section parsing, a macro can change the meaning of a section header. These are not hypothetical; they are documented behaviors.

The practical result: the spec you read is not the spec RPM executes. `rpmspec --parse` exists precisely because the source text is not the semantic artifact.

### 3. `%files` is a pattern list, not a manifest

`%files` supports globs, `%exclude`, `%dir`, `%config`, `%ghost`, `%attr`, `%caps`, `%verify`, `%doc`, `%license`, `%artifact`, `%missingok`, and more. It is powerful. But:

- A glob that matches nothing is tried literally, and the error appears late.
- Unpackaged files produce warnings, not a hard failure (depending on policy).
- Two subpackages can claim the same path; the conflict is discovered during packaging, not during planning.
- The only way to know what will be packaged is to run the build.
- File ownership is spread across multiple `%files` sections, and the relationship between them is implicit.

CRUX's `.footprint` and Debian's `*.install` are attempts to make this explicit. RPM would benefit from a computed manifest with a totality check: every staged file is claimed exactly once, or the build fails.

### 4. Subpackages are a second preamble in the same file

`%package` starts a new preamble section; `%description` starts another. Most tags can be repeated, some are inherited, some are not. The result is duplication and subtle inheritance rules. There is no single place that says "these are the outputs of this source package".

Arch's split functions, Alpine's `subpackages`, Void's split functions, Yocto's `PACKAGES`/`FILES`, and Flatpak's modules all model multiple outputs more directly.

### 5. Dependency generation is opaque and sometimes two-pass

RPM's automatic dependency generators are powerful: ELF sonames, pkg-config, Python, Perl, Ruby, etc. But:

- The generated dependencies are not visible in the spec; you need `rpmspec -q --requires` or a built package.
- Generators are configured through macro files and file attributes, which are global state.
- Dynamic build dependencies require `%generate_buildrequires`; if dependencies are missing, rpmbuild writes a `.buildreqs.nosrc.rpm`, you install it, and restart. That is a two-pass build protocol imposed by the tool.
- `BuildRequires` has no distinction between tools for the build machine and libraries for the target sysroot.

Gentoo's `DEPEND`/`BDEPEND`/`RDEPEND`/`IDEPEND`, Void's `hostmakedepends`/`makedepends`, pkgsrc's `TOOL_DEPENDS`/`BUILD_DEPENDS`, and Conan's build/host contexts all model this better.

### 6. Cross-compilation is an afterthought

`rpmbuild --target` changes the target platform, but the spec language has no first-class concept of:

- build machine vs. host machine vs. target machine,
- a sysroot,
- tools that run on the build machine vs. libraries that link for the target,
- target-specific file paths and dependency generators.

Packagers work around this with macros, `%_target_platform`, `--define`, and custom toolchains. Yocto, Buildroot, OpenWrt, pkgsrc, Void and Conan treat this as a core design concern.

### 7. Tests are optional and under-modelled

`%check` exists, but:

- It is optional.
- It usually runs build-tree tests, not tests against the installed package.
- Test-only dependencies are mixed into `BuildRequires`.
- There is no standard way to run the test suite in a disposable installed root.
- Distros frequently disable tests to unblock builds, and the spec does not record why.

Debian's autopkgtest, Homebrew's `test do`, Spack's `spack test`, and Yocto's test/QA infrastructure are ahead here.

### 8. Scriptlets are arbitrary root shell

RPM's scriptlets are necessary for system integration, but they are:

- arbitrary shell (or another interpreter) executed as root,
- ordered by dependency heuristics,
- hard to test,
- hard to review,
- a common source of upgrade bugs,
- duplicated across packages for systemd, sysusers, tmpfiles, ldconfig, alternatives, icon caches, etc.

RPM has improved this with `%systemd_*` macros, `sysusers.d` integration (4.19), file triggers, and the `unshare` plugin for scriptlet isolation (4.20). But the spec language still presents scriptlets as the primary integration mechanism, and macros only hide the shell.

### 9. No hermeticity, no lockfile

`rpmbuild` does not isolate the build. `mock`/`koji` provide a chroot, which is much better, but:

- the chroot is a mutable package set, not a content-addressed input,
- build dependencies are resolved at build time,
- network access is possible unless the builder blocks it,
- sources are pinned by the SRPM, but the build environment and toolchain are not,
- there is no `--locked` mode that refuses drift.

Nix, Guix, Bazel, Spack and Conan all provide a lock or derivation that says "these exact inputs produce this exact output". RPM has an SRPM, which is a source snapshot, not a build-environment lock.

### 10. Changelog and release are bolted on

`%changelog` is a free-text block with a parser. Release is a manual integer. `rpmautospec` improves this by generating `%autorelease` and `%autochangelog` from git history, but it depends on mutable git state at build time. A modern format should resolve release/changelog into the lock, so the build is reproducible outside the original git checkout.

### 11. Error messages are macro-expanded soup

When a spec fails, the error often points into expanded text that does not exist in the source file. Debugging requires `rpmspec --parse`, `--define` experiments, and knowledge of macro precedence. A typed format can produce errors with a file, line, field path, expected type, and a suggestion.

### 12. The format has no schema

RPM tags are documented, but there is no machine-readable schema that a linter, editor, or CI system can use to validate a spec before building. `rpmlint` checks the *output*; it does not validate the input language. New packagers learn by copying specs and discovering rules through failures.

---

## What RPM genuinely does better

It is important not to throw this away:

1. **File-level metadata.** `%config(noreplace)`, `%license`, `%doc`, `%ghost`, `%caps`, `%attr`, `%verify`, `%artifact`, `%missingok`, `%dir`. Debian has conffiles and `.install` files, but RPM's per-file metadata is richer.
2. **Rich dependencies.** `Requires: (pkgA or pkgB)`, `if`, `if else`, `with`, `without`, `unless`. This is more expressive than Debian's alternatives and far more expressive than Arch/Alpine.
3. **Weak dependencies.** `Recommends`, `Suggests`, `Supplements`, `Enhances`, with well-defined solver semantics.
4. **File triggers.** Efficient, declarative-ish triggers on file paths, far better than per-package scriptlets for caches.
5. **Subpackages.** One source package can produce many binary packages, with shared build work.
6. **Debuginfo.** Automatic `-debuginfo`/`-debugsource` generation, build-ids, `debugedit`.
7. **Signed packages and a real database.** RPM's signature and transaction model is mature.
8. **`%autosetup`.** Patch series, VCS backends (`git`, `hg`, `quilt`, `patch`, `git_am`, `gendiff`), `%autopatch` ranges.
9. **`BuildSystem` (RPM ≥ 4.20).** Declarative build-system selection with append/prepend and `BuildOption`. This is the right direction.
10. **`%generate_buildrequires` (RPM ≥ 4.15).** Dynamic build dependency discovery, despite the awkward two-pass protocol.
11. **`%bcond` (RPM ≥ 4.17).** A nicer conditional-build primitive.
12. **Timestamp policy (RPM ≥ 4.20).** `%build_mtime_policy` is a real reproducibility improvement.
13. **`sysusers.d` integration (RPM ≥ 4.19).** A declarative alternative to hand-written user/group scriptlets.
14. **RPM 6.** v4/v6 package support, multiple OpenPGP signatures, PQC keys, signature enforcement by default, and better key identity. The package format is alive and improving.

Sources: [RPM dependencies](https://rpm.org/docs/6.1.x/manual/dependencies.html), [boolean dependencies](https://rpm.org/docs/latest/manual/boolean_dependencies.html), [autosetup](https://rpm.org/docs/latest/manual/autosetup.html), [timeline](https://rpm.org/timeline), [RPM 6.0 release notes](https://rpm.org/releases/6.0.0).

---

## What Wolfi, melange and apko teach RPM

Wolfi is the most interesting recent counter-example to "a distro package format must be a spec-like language". It is a minimal, rolling undistro for containers; packages are APKs built by melange; images are composed by apko. It is not a general-purpose desktop distro, so the comparison is not one-to-one, but three design choices are directly relevant.

### 1. Pipelines instead of phases

Melange replaces `%prep`/`%build`/`%install` with an ordered pipeline:

```yaml
pipeline:
  - uses: fetch
    with:
      uri: https://ftp.gnu.org/gnu/hello/hello-${{package.version}}.tar.gz
      expected-sha256: ...
  - uses: autoconf/configure
  - uses: autoconf/make
  - uses: autoconf/make-install
  - uses: strip
```

Each `uses` step is a named, reusable pipeline with its own declared needs (`needs.packages`, `needs.capabilities`). `runs:` is the shell escape hatch. This is better than RPM's phases in three ways:

- common steps are shared and reviewable, not copied into every spec;
- the build environment needs are declared by the steps, not inferred from a giant `BuildRequires` list;
- the pipeline is data, so tooling can show it before execution.

RPM 4.20's `BuildSystem` is a move in this direction, but it still expands to macros and shell. A successor should make named steps the primary model and shell the exception.

### 2. The build environment is a declared package set

Melange's `environment.contents` lists repositories, keyrings and packages. The guest is assembled by apko. There is no ambient host toolchain and no implicit `PATH`. This is much closer to a hermetic build than a mock chroot, though it still lacks a content-addressed lock.

RPM would benefit from the same declaration: the build root should be a resolved, recorded package set, not "whatever is installed in the mock root when the build starts".

### 3. Composition should not run arbitrary commands

apko deliberately has no `RUN`. An image is a declarative function of packages, accounts, paths, environment and entrypoint. This is what makes bitwise reproducibility and complete SBOMs credible.

RPM's ecosystem is moving toward image-based OS delivery (bootc, rpm-ostree, OSBuild), but the composition layer often falls back to Containerfiles and shell. The lesson is not "RPM should build images"; it is "if you separate package build from image composition, make the composition layer pure".

### 4. Tests should use a fresh guest

Melange's `test` pipeline builds a fresh apko guest containing the package under test plus declared test dependencies. That is a better model than `%check`, which runs in the build tree and is often disabled.

### 5. Evidence is an output

Melange and apko emit SBOMs; the Wolfi repository is signed. The package format does not treat SBOM/provenance as a separate afterthought.

### What Wolfi/melange/apko still lack

- **No lockfile.** `environment.contents` is a package list, not an exact closure.
- **No RPM-style per-file metadata.** No `%config`, `%caps`, `%ghost`, `%verify` depth.
- **A simpler dependency model.** APK does not have RPM's rich/weak dependency grammar or file-trigger depth.
- **YAML plus `runs:` shell.** The escape hatch is still where complexity hides.
- **A curated, container-focused package set.** Not a general-purpose distro.

The implication for a successor is clear: take the pipeline model, declared build environment, fresh-guest tests and evidence-as-output; add the lock, typed file manifest and RPM runtime semantics that melange/apko do not have.

Sources: [melange build file](https://github.com/chainguard-dev/melange/blob/main/docs/BUILD-FILE.md), [melange build process](https://github.com/chainguard-dev/melange/blob/main/docs/BUILD-PROCESS.md), [melange testing](https://github.com/chainguard-dev/melange/blob/main/docs/TESTING.md), [apko file format](https://github.com/chainguard-dev/apko/blob/main/docs/apko_file.md), [Wolfi](https://github.com/wolfi-dev/os).

---

## Why incremental RPM changes are not enough

RPM 4.20 and 6.x have added exactly the features a modern packager wants:

- declarative build systems,
- dynamic specs,
- per-build directories,
- timestamp policy,
- `sysusers.d`,
- spec-local file attributes and generators,
- scriptlet isolation,
- debuginfo improvements.

These are good. But they are still expressed *inside the spec language*:

- `BuildSystem: cmake` is a tag, but customisation is still `%build -p`, `%install -a`, and `BuildOption`.
- `%bcond` is still an untyped macro.
- Dynamic specs still generate spec text.
- `sysusers.d` is still a file plus macro behaviour, not a typed action.
- Timestamp policy is still a global macro.

The remaining problems are language-level:

- parse-time evaluation,
- no schema,
- no separation of metadata/build/files/policy,
- no total file manifest,
- no lockfile,
- no hermetic execution,
- no first-class tests/provenance.

You cannot fix those by adding more tags without breaking compatibility. A clean-sheet frontend can.

---

## Requirements for a successor

A replacement should satisfy these requirements. They become the design principles in the next document.

### R1. Deterministic elaboration

`Plan = f(recipe, profile, feature set, lock)` must be a pure function. No host introspection, no mutable macro files, no network during parsing.

### R2. Typed, closed schema

Unknown fields are errors. Every value has a type and a range. Errors point to a file, line, field path and expected shape.

### R3. Separation of concerns

- **Recipe**: what the software is and how to build it.
- **Profile**: distribution policy (paths, flags, allowed actions, required tests).
- **Lock**: exact sources, features, dependencies, toolchain and manifest digests.
- **Plan**: the resolved step graph, dependency graph and output manifest.
- **Evidence**: SBOM, provenance, test results, reproducibility report.

### R4. Total file ownership

Every file in the staging tree is claimed by exactly one output, or explicitly ignored. The planner computes this before packaging. Duplicate claims and unowned files are hard errors.

### R5. First-class dependencies

Separate `tools`, `host`, `target`, `tests`, `run`. Support version constraints, alternatives, weak dependencies, virtual providers, and ABI-triggered rebuilds. Make automatic generation visible and overridable.

### R6. First-class features

Features are data with dependencies, build options, patches, files and constraints. Invalid combinations fail before the build. The enabled set is recorded in the lock.

### R7. First-class outputs

Multiple outputs are an `outputs:` list, each with metadata, dependencies, file selectors and actions. No second preamble, no implicit naming rules.

### R8. Quarantined escape hatches

Common work is declarative. Unusual work is an inline pipeline step with an `argv` array or a shell fragment, declared tools, and visibility in the plan. Raw shell is allowed, linted and visible.

### R9. Hermetic execution by default

Network off after fetch. Only declared tools and dependencies mounted. Build root is fresh. `SOURCE_DATE_EPOCH` and deterministic ordering are set by the tool. `--no-sandbox` is explicit and loud.

### R10. Locked, reproducible builds

`--locked` refuses drift. A rebuild in a different directory produces identical output, verified by the tool. The lock records source hashes, feature set, profile digest, toolchain and output manifest digests.

### R11. Tests and evidence are outputs

Build tests and installed-package tests are declared with their own dependencies. SBOM and provenance are generated from the plan and the actual payload. The tool can attest what it built.

### R12. RPM format fidelity

The result is a normal RPM: headers, payload, file metadata, scriptlets, rich/weak deps, signatures. Existing RPM consumers must not need to change.

### R13. Migration is possible

Import simple specs and migrate package by package. Native SRPMs are rebuilt with `rpmx`; there is no spec export or legacy rebuild path. Do not require a flag day.

### R14. Matrices and templates are part of the model

One recipe must be able to describe a family of builds (for example, one package per supported Python version) without `%if` chains or duplicated files. A matrix is typed data: axes, exclusions, a cell limit, and one build cell per combination, each with its own dependencies, build root, outputs and SRPM. A template is a closed, typed variable scope, not a macro language. Melange's `data`/`range` proves the output side of this; the design in [`11-matrices.md`](11-matrices.md) generalizes it to build cells, and [`12-templates.md`](12-templates.md) defines the substitution rules.

---

## The core bet

The core bet is:

> Most of the value of a packaging format comes from a small, typed model plus a deterministic planner, not from the expressiveness of its macro language.

If that is true, a recipe can be *less* powerful than a spec and still package *more* software safely, because the hard parts—dependencies, features, files, policy, tests, provenance—are modelled directly instead of being encoded in shell and macros.

The next document turns this into a concrete design.
