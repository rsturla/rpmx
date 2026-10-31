# The packaging-definition landscape

## Method and dimensions

I looked at primary documentation for 30+ systems and compared them on the dimensions that actually determine whether a packaging format is pleasant and safe to use:

1. **Definition language** — data, shell, Make, Python, Ruby, Tcl, Starlark, Nix, Scheme.
2. **Separation of concerns** — metadata vs. build logic vs. file lists vs. distribution policy.
3. **Build model** — fixed phases, free-form script, task graph, action graph.
4. **Dependency model** — build/host/target/test/run separation, version constraints, virtuals, weak/rich dependencies.
5. **Variants and features** — how optional functionality is expressed and constrained.
6. **Composition and reuse** — classes, eclasses, helpers, modules, layers, overlays, mixins.
7. **Isolation and reproducibility** — sandboxing, network policy, locked inputs, content addressing, rebuild verification.
8. **Output model** — one package or many, file ownership, splitting, subpackages.
9. **Tests, QA and evidence** — test phases, linters, SBOMs, provenance, reproducibility checks.
10. **Ergonomics** — learnability, error messages, tooling, debuggability.

The comparison is intentionally asymmetric: RPM is judged as a *general-purpose distribution package definition*, while some of the others are judged in their own domain (embedded images, C++ dependencies, desktop apps). A system can be excellent at its job and still contain a lesson for RPM.

---

## Comparison matrix

### A. Distribution-native package definitions

| System | Definition | Composition | Variants / features | Isolation / reproducibility | Outputs |
|---|---|---|---|---|---|
| **RPM spec** | Custom text format + macro preprocessor + shell | Macros, `%include`, distro macro packages | `%bcond`, `%if`, macro conditionals | `rpmbuild` buildroot; network allowed; no lockfile; one build dir per package since 4.20 | Binary RPMs, subpackages, SRPM |
| **Debian** | `debian/control` + `debian/rules` (Make) + `*.install` + `debian/copyright` + `debian/tests` | `dh` sequencer, addons, override/hook targets | Build profiles, `Build-Depends-{Indep,Arch}`, `Multi-Arch` | `dpkg-buildpackage`; sbuild/pbuilder for clean chroots; no lockfile | `.deb`, `.udeb`, source package |
| **Arch** | `PKGBUILD` (Bash) | Shell functions, split-package functions | `arch=`, `options`, per-arch arrays | `makepkg`; clean chroot via devtools for official builds; no lockfile | `.pkg.tar.zst`, split packages |
| **Alpine** | `APKBUILD` (POSIX shell) | `default_*` split functions, `subpackages` | `depends_*`, `options`, arch allow/deny lists | `abuild`; clean chroot; no lockfile | `.apk`, subpackages |
| **Gentoo** | `ebuild` (Bash) + EAPI + eclasses | Eclasses, profiles, `USE` flags | `IUSE`, `REQUIRED_USE`, `USE_EXPAND`, slots/subslots | Portage sandbox; network restricted in phases; no lockfile | `.tbz2`/`.gpkg.tar`, slots |
| **Void** | `template` (Bash) | `build_style` scripts, build helpers, subpackages | `build_style`, `configure_args`, arch patterns | `xbps-src` chroot; cross-aware deps; no lockfile | `.xbps`, subpackages |
| **Solus** | `package.yml` (YAML) + Bash phase scripts | `ypkg` macros, patterns, auto-split | `optimize`, `emul32`, `clang`, `networking` | `solbuild` overlayfs chroot; no lockfile | `.eopkg`, subpackages |
| **FreeBSD ports** | `Makefile` + `pkg-plist` + `distinfo` + `pkg-descr` | `bsd.port.mk`, `USES`, options helpers, flavors | `OPTIONS_DEFINE`, `FLAVORS` | `poudriere` clean jails; no lockfile | `.pkg`, flavors |
| **pkgsrc** | `Makefile` + `PLIST` + `distinfo` | `bsd.pkg.mk`, `buildlink3.mk`, `USE_TOOLS` | `PKG_OPTIONS`, alternatives | `pkg_comp`/chroot; buildlink isolation; no lockfile | `.tgz`, binary packages |
| **OpenWrt** | `Makefile` with `Package/foo` blocks | `BuildPackage`, `Build/InstallDev`, feeds | `CONFIG_*`, `DEPENDS` conditionals | SDK/cross toolchain; no per-package sandbox; no lockfile | `.ipk`/`.apk` |
| **Slackware/SBo** | `*.SlackBuild` (Bash) + `*.info` | Shell templates, `REQUIRES` metadata | Manual flags | None by default; no dependency solver; no lockfile | `.tgz`/`.txz` |
| **CRUX** | `Pkgfile` (Bash) + `.footprint` | Shell, `prt-get` aliases | Manual flags | `pkgmk`; optional rootlesskit; no lockfile | `.pkg.tar.*` |

### B. Functional, graph-based and cross-ecosystem definitions

| System | Definition | Composition | Variants / features | Isolation / reproducibility | Outputs |
|---|---|---|---|---|---|
| **Nix** | Nix expression, `stdenv.mkDerivation` | Functions, overlays, `override`/`overrideAttrs`, `callPackage` | Function arguments, `override`, optional attrs | Hermetic sandbox, content-addressed store, fixed-output derivations, binary cache | Store paths, multiple outputs |
| **Guix** | Scheme, `package` records, G-expressions | Inheritance, package transformations, services | Fields and transformations | Hermetic builds, content-addressed store, `guix challenge` | Store paths, packs, system images |
| **Spack** | Python `package.py` | Class inheritance, build-system classes, mixins | Specs, variants, virtuals, `when=` | Concretization + `spack.lock`; build caches; not fully hermetic by default | Install trees, modules, containers |
| **Conan** | Python `conanfile.py` | Python requires, base classes, generators | Settings, options, `package_id`, profiles | Build/host contexts, lockfiles, revisions, binary cache | Package archives in cache |
| **vcpkg** | `vcpkg.json` + `portfile.cmake` | Port helpers, overlay ports | Features, triplets, baseline pinning | ABI hashes, binary caching, no strong sandbox | Installed tree / cache |
| **Bazel** | Starlark `BUILD`/`.bzl` | Rules, toolchains, platforms, aspects | Build settings, configurable attributes | Hermetic action graph, sandboxing, remote cache/execution | Arbitrary artifacts, incl. RPM/deb via rules |

### C. Application, image and platform definitions

| System | Definition | Composition | Variants / features | Isolation / reproducibility | Outputs |
|---|---|---|---|---|---|
| **Yocto** | `.bb` recipe + `.bbclass` + layers | Classes, layers, `bbappend`, `BBCLASSEXTEND` | `PACKAGECONFIG`, overrides, multiconfig | BitBake sandbox, shared-state cache, license/SBOM checks | Packages, rootfs, SDK, images |
| **Buildroot** | `.mk` + `Config.in` | Package infra, hooks, board files | Kconfig options | Cross toolchain; no binary package manager; no fine-grained rebuild graph | Rootfs, images, SDK |
| **Flatpak** | JSON/YAML manifest | Modules, shared modules, runtimes/SDKs | Buildsystem options, `finish-args` | `flatpak-builder` sandbox, runtime/SDK pinning, no lockfile | `.flatpak` bundle/repo |
| **Snap** | `snapcraft.yaml` | Parts, plugins, extensions | `build-packages`, `stage-packages`, platforms | Snapcraft provider/LXD, confinement, no lockfile | `.snap` |
| **Homebrew** | Ruby `Formula` DSL | Helpers, `resource`, `patch`, `uses_from_macos` | `depends_on` flags, `on_os`, `on_arch` | `superenv`, sandbox on macOS, bottles, no lockfile | Keg, bottle |
| **MacPorts** | Tcl `Portfile` | `PortGroup`, subports, variants | Variants, `depends_*` | `destroot`; trace mode; binary archives | `.pkg`/archive |

### D. Modern undistro and image-first definitions

| System | Definition | Composition | Variants / features | Isolation / reproducibility | Outputs |
|---|---|---|---|---|---|
| **Wolfi / melange** | YAML package file + ordered pipeline | Named pipelines (`uses`), subpackages, `vars`/`data`, `range:` matrix expansion over subpackages | `target-architectures`, `options`, per-subpackage pipelines/tests, `var-transforms` | bubblewrap guest built by apko, QEMU for foreign arches, SBOM, no lockfile | `.apk`, SBOM |
| **apko** | YAML image config | `include`, package list, `paths`, `accounts`, layering | `archs`, layering budget, entrypoint | No `RUN`; bitwise reproducible; SBOM; package repo state | OCI image |
| **Chisel** | YAML slice definitions | Slices within packages, cross-slice dependencies | Per-package slice selection | Extracts from `.deb`, minimal rootfs, manifest | Rootfs slices |
| **OSBuild** | JSON manifest + stages | Stages, sources, pipelines | Per-stage options | Pipeline build; image reproducibility depends on stages | OS images |
| **mkosi** | INI config + scripts | Profiles, includes, build/post-install scripts | Distribution/release, package sets | Builds via distro package manager; not hermetic by default | Disk images |
| **bootc / rpm-ostree** | Containerfile + RPM packages | Base images, layers, `rpm-ostree` overrides | Image tags, package overrides | Image-based updates; build provenance depends on container build | Bootable OCI images |
| **BlueBuild** | YAML recipe + modules | Modules, base images | `rpm-ostree`/`dnf`/`copr`/scripts | Image build; not a package definition | Bootable images |
| **Rockcraft** | YAML + parts/plugins | Parts, extensions | Platforms | LXD/multipass build; OCI output | OCI image |

---

## Per-system notes: what each does better and worse than rpmbuild

### 1. RPM spec (the baseline)

**Better than most:** file-level metadata (`%config`, `%license`, `%doc`, `%caps`, `%ghost`, `%attr`, `%verify`), rich and weak dependencies, file triggers, subpackages, debuginfo generation, signed packages, a real dependency solver ecosystem (dnf/libsolv), and `%autosetup`/`%generate_buildrequires`/`BuildSystem` as recent improvements.

**Worse than most:** one file mixes metadata, policy, build shell and file globs; parse-time macro expansion makes semantics environment-dependent; `%if` distro conditionals spread everywhere; no schema; no lockfile; no hermeticity; tests are optional; file ownership is a glob rather than a manifest; scriptlets are arbitrary root shell; cross-compilation is weak; error messages are macro-expanded soup.

**Lesson:** keep the runtime semantics, replace the authoring model.

---

### 2. Debian

Debian's packaging is spread across `debian/control`, `debian/rules`, `debian/*.install`, `debian/copyright`, `debian/tests`, `debian/watch`, and `debian/patches/`. That looks like sprawl, but each file has one job.

**Better than RPM**

- **Metadata is separate from build logic.** `debian/control` is a declarative control file; `debian/rules` is the build program. RPM puts both in one spec.
- **A composable pipeline with precise override points.** `dh` runs a named sequence; `override_dh_*`, `execute_before_dh_*` and `execute_after_dh_*` let you change one step without rewriting the world. This is much better than RPM's monolithic `%install`.
- **Dependency classes are explicit.** `Build-Depends`, `Build-Depends-Indep`, `Build-Depends-Arch`, `Build-Conflicts*` distinguish what is needed for which artifact. RPM has only `BuildRequires`.
- **Build profiles** (`<nocheck>`, `<cross>`, custom profiles) are a standard way to vary a build without editing the package.
- **`Multi-Arch`** is a first-class model for co-installability.
- **`Built-Using`** records source packages incorporated into a binary, which is useful for license compliance.
- **Tests are a standard artifact.** `debian/tests/control` + autopkgtest define how to test an *installed* package, not just `make check`.
- **Machine-readable copyright** (DEP-5) is far ahead of RPM's `License:` tag and `%license` files.
- **`debian/*.install`** makes file placement per binary package explicit, rather than one `%files` block per subpackage.
- **`dpkg-buildflags`** centralizes hardening and optimization flags.

**Worse than RPM**

- **Make is the build language.** `debian/rules` is a Makefile, with tab sensitivity and awkward quoting. `dh` hides it, but custom logic falls back to Make.
- **No standard build-system declaration.** `dh_auto_*` guesses; RPM 4.20's `BuildSystem:` is cleaner.
- **Many small files** and a lot of policy knowledge. The `debian/` directory is a mini-program.
- **No lockfile** for build dependencies or sources; reproducibility depends on archive state and snapshot pinning.
- **Weak cross-compilation** and no first-class sysroot model.
- **Maintainer scripts** are still arbitrary shell, same as RPM scriptlets.
- **Source package formats** (`3.0 (quilt)`, native, multiple tarballs) are powerful but easy to get wrong.

**Lesson:** separate metadata, build plan and per-output file lists; make the pipeline a named sequence with hooks; add build profiles and installed-package tests.

Sources: [Debian Policy](https://www.debian.org/doc/debian-policy/), [dh(1)](https://manpages.debian.org/unstable/debhelper/dh.1.en.html), [Machine-readable copyright](https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/), [Debian maint-guide](https://www.debian.org/doc/manuals/maint-guide/).

---

### 3. Arch Linux (PKGBUILD)

**Better than RPM**

- **Radically simple.** A `PKGBUILD` is a Bash script with a small set of arrays and four functions: `prepare()`, `build()`, `check()`, `package()`. A newcomer can read one in five minutes.
- **Split packages are straightforward.** `pkgname=(foo libfoo)` plus `package_foo()` / `package_libfoo()`; per-split overrides for metadata and dependencies.
- **Good source model.** Multiple sources, per-architecture sources, checksums, PGP signatures, VCS sources pinned by branch/commit/tag, and `pkgver()` for dynamic versions.
- **`options` array** gives per-package control over stripping, docs, libtool, static libs, ccache, distcc, LTO and build flags.
- **`.SRCINFO`** is a generated, machine-readable projection of the PKGBUILD, used by the AUR and tooling.
- **`namcap`** provides packaging linting.

**Worse than RPM**

- **Turing-complete Bash is the definition language.** Anything can happen; static analysis is limited.
- **No real dependency solver semantics** beyond package names and simple version constraints; no rich/weak dependencies, no virtual providers beyond `provides`.
- **No clean-room build by default.** Official builds use `devtools`/`makechrootpkg`, but `makepkg` itself trusts the host.
- **No cross-compilation model.**
- **No reproducible-build policy**; timestamps, ordering and environment are the packager's problem.
- **`package()` is a shell free-for-all**, so file ownership is only visible after the build.
- **No stable ABI/soname model**; rebuilds are coordinated by humans.

**Lesson:** keep the readability and the split-package ergonomics; replace Bash with data plus typed hooks; add a solver-grade dependency model.

Source: [PKGBUILD(5)](https://pacman.archlinux.page/PKGBUILD.5.html).

---

### 4. Alpine Linux (APKBUILD)

**Better than RPM**

- **Build styles and default split functions.** `default_dev`, `default_doc`, `default_libs`, `default_static`, `default_systemd`, `default_bashcomp`, etc. encode common packaging knowledge. RPM has `%files` macros, but not a standard split policy.
- **Subpackages are a first-class list** with split functions and optional arch overrides.
- **Cross-awareness in dependencies.** `makedepends_build` (CBUILD) vs. `makedepends_host` (CHOST), plus `CBUILD`/`CHOST`/`CTARGET`.
- **`install_if`** lets a subpackage be pulled in automatically when a condition holds, which is a neat alternative to hard dependencies.
- **Triggers** are declarative paths plus scripts.
- **SPDX license expressions** and `sha512sums`.
- **`options`** covers `!check`, `net`, `!strip`, `suid`, `setcap`, `textrels`, `libtool`, etc.
- **`abuild` checks** enforce policy at build time.

**Worse than RPM**

- **POSIX shell is the language**, with the same static-analysis and quoting problems as Arch.
- **`default_*` functions are conventions, not a schema**; overriding them requires knowing the framework.
- **No rich/weak dependency model** comparable to RPM's.
- **No hermetic sandbox or lockfile.**
- **Documentation is thinner** than Debian/Gentoo/RPM.
- **Subpackage metadata is set by mutating variables inside split functions**, which is hard to reason about.

**Lesson:** standard split policies and cross-aware dependency scopes are worth stealing; encode them as data and validate them.

Source: [APKBUILD(5)](https://github.com/alpinelinux/abuild/blob/master/APKBUILD.5.scd).

---

### 5. Gentoo (ebuild)

**Better than RPM**

- **The richest feature/variant model in distro packaging.** `IUSE`, `REQUIRED_USE`, `USE_EXPAND`, per-package `package.use`, profiles, and `use_enable`/`use_with` helpers.
- **`REQUIRED_USE` is a real constraint language**: `foo? ( !bar )`, `^^ ( a b c )`, `|| ( a b c )`, `?? ( a b c )`.
- **Dependency classes are excellent**: `DEPEND` (CHOST), `BDEPEND` (CBUILD), `RDEPEND`, `IDEPEND` (install-time), `PDEPEND` (post-merge).
- **Slots and subslots** model parallel-installable versions and ABI-triggered rebuilds.
- **Eclasses** are a mature reuse mechanism (multilib, python, cmake, meson, etc.).
- **`RESTRICT` and `PROPERTIES`** make build assumptions explicit (network, fetch, test, strip, userpriv).
- **`eapply`, `econf`, `emake`, `dodoc`, `dobin`, `einstalldocs`** give consistent helpers.
- **Profiles** separate distro policy from package recipes better than RPM macros.

**Worse than RPM**

- **Enormous complexity.** EAPI versions, eclass interactions, USE flag combinatorics, profile stacking, and a steep learning curve.
- **Bash plus eclasses is still a dynamic language**; reasoning about an ebuild requires sourcing it.
- **USE flag explosion** is a real maintenance and support burden.
- **No hermetic build by default**; Portage sandboxing helps but does not give Nix-style purity.
- **No lockfile**; dependency resolution is global and changes over time.
- **Slots/subslots are powerful but subtle**, and rebuild propagation is a source of pain.

**Lesson:** features should be typed data with constraints; dependency scopes should be explicit; slots/subslots are a good model for ABI compatibility.

Source: [Gentoo DevManual](https://devmanual.gentoo.org/), [variables](https://devmanual.gentoo.org/ebuild-writing/variables/), [USE flags](https://devmanual.gentoo.org/general-concepts/use-flags/).

---

### 6. Void Linux (xbps-src)

**Better than RPM**

- **`build_style` is a clean build-system adapter**: `gnu-configure`, `cmake`, `meson`, `cargo`, `go`, `python3-pep517`, `perl-module`, `qmake`, etc.
- **Subpackages and split helpers** (`vinstall`, `vmove`, `vcopy`, `vman`, `vdoc`, `vlicense`, `vcompletion`) are pragmatic and readable.
- **Cross-aware dependencies**: `hostmakedepends` vs. `makedepends` vs. `checkdepends`.
- **SONAME tracking** via `common/shlibs` maps `libfoo.so.1` to a package and minimum version.
- **`reverts`** is a genuinely useful feature: a new package version can declare that it reverts broken versions.
- **`alternatives`, `conf_files`, `make_dirs`, `register_shell`, `dkms_modules`** are typed integration points.
- **`xlint`** lints templates.
- **`vsed`** checks that a substitution actually changed the file, which catches silent patch rot.

**Worse than RPM**

- **Bash templates**; same dynamic-language issues.
- **No rich dependency model**; dependencies are package names plus simple version ops.
- **No lockfile or hermetic build**; the chroot is clean but not content-addressed.
- **Small ecosystem and documentation** compared with RPM/Debian.
- **Split functions still mutate state** and are order-sensitive.

**Lesson:** build-style adapters, SONAME tracking, revert metadata, and typed integration points are all worth incorporating.

Source: [Void xbps-src manual](https://github.com/void-linux/void-packages/blob/master/Manual.md).

---

### 7. Solus (package.yml)

**Better than RPM**

- **Declarative YAML for metadata plus named script phases** (`setup`, `build`, `install`, `check`, `profile`). This is the closest existing thing to "declarative by default, script by exception".
- **`patterns`** give fine-grained file placement into subpackages.
- **`autodep`** and **`libsplit`** automate dependency and library splitting.
- **`optimize`** is a typed list of optimization presets (`speed`, `size`, `lto`, `thin-lto`, `icf-safe`, etc.) instead of raw flags.
- **`emul32`, `clang`, `networking`** are explicit build modes.
- **`%` macros** (`%configure`, `%make`, `%cmake`, `%meson`, `%cargo_*`, `%pyproject_*`) are consistent helpers.
- **`solbuild`** uses overlayfs and a clean base image.

**Worse than RPM**

- **YAML plus Bash**; no schema validation is evident in the docs, and scripts are still free-form.
- **Small ecosystem**; less solver sophistication.
- **No rich/weak dependencies or file triggers** comparable to RPM.
- **No lockfile/hermeticity.**
- **Macros are string substitution**, so quoting and escaping remain issues.

**Lesson:** a declarative metadata file plus named script phases is a good ergonomic middle ground; typed optimization presets are better than raw flag soup.

Source: [Solus package.yml](https://help.getsol.us/docs/packaging/package.yml).

---

### 8. FreeBSD ports

**Better than RPM**

- **`USES` is a composable framework**: `cmake`, `meson`, `cargo`, `python`, `perl5`, `ssl`, `iconv`, `shebangfix`, `desktop-file-utils`, etc. Each `USES` macro encodes a policy.
- **Options framework** with `OPTIONS_DEFINE`, `OPTIONS_DEFAULT`, `opt_*` variables, and per-option dependencies/patches/configure flags.
- **Flavors** (`FLAVORS`, `FLAVOR`) support multiple builds from one port (e.g., Python versions, SSL backends).
- **`pkg-plist` is an explicit file manifest**, with keywords for `@sample`, `@dir`, `@postexec`, etc.
- **`distinfo`** records hashes separately from the Makefile.
- **`poudriere`** gives clean-jail, reproducible-ish builds and bulk package testing.
- **`portlint`, `portclippy`, `portfmt`** provide linting and formatting.
- **`MASTER_SITES`/mirrors** are a first-class source model.

**Worse than RPM**

- **Makefile syntax and `bsd.port.mk` complexity.** The framework is powerful but arcane.
- **Options and flavors multiply combinatorially** and can produce many untested builds.
- **No rich/weak dependencies**; simple package/version constraints.
- **No hermetic build by default**; `poudriere` is the clean path.
- **No lockfile.**
- **`pkg-plist` maintenance** is manual and error-prone (though `make makeplist` helps).

**Lesson:** `USES` is a good model for build-system/framework adapters; flavors are a good model for multi-version builds; an explicit plist plus clean-room builder is a strong combination.

Source: [FreeBSD Porter's Handbook](https://docs.freebsd.org/en/books/porters-handbook/book/).

---

### 9. NetBSD pkgsrc

**Better than RPM**

- **`buildlink3.mk` is excellent dependency hygiene.** It creates a wrapper tree so a package can only see headers/libraries from declared dependencies, preventing "automagic" host contamination.
- **`TOOL_DEPENDS` vs. `BUILD_DEPENDS`** distinguishes native tools from target libraries, which is essential for cross builds.
- **`USE_TOOLS`** declares tool requirements (gmake, perl, awk, sed, etc.) in a portable way.
- **`PKG_OPTIONS`** is a mature options framework with global and per-package defaults.
- **`pkglint`** is a strong, opinionated linter.
- **`print-PLIST`** generates a plist from the installed tree.
- **Cross-compilation support** is a first-class concern.

**Worse than RPM**

- **Make plus a huge framework.** The learning curve is steep.
- **`buildlink3.mk` is complex** and requires understanding wrapper semantics.
- **No rich/weak deps**, no file triggers, no scriptlet model as rich as RPM's.
- **No lockfile/hermeticity.**
- **Slow builds and a small ecosystem** compared with RPM.

**Lesson:** buildlink-style isolation is a strong alternative to "just install all build deps into a chroot"; tool vs. target dependencies should be explicit; `print-PLIST` suggests the value of generating a manifest from the staged tree and then reviewing it.

Source: [pkgsrc guide](https://www.netbsd.org/docs/pkgsrc/creating.html).

---

### 10. OpenWrt

**Better than RPM**

- **Very explicit package definitions** using `define Package/foo` blocks, with metadata, dependencies, and per-package install functions.
- **`Build/InstallDev`** separates the development/staging install from the runtime package install.
- **Kconfig-style `CONFIG_*`** makes feature selection explicit and visible in `.config`.
- **`PKG_CONFIG_DEPENDS`** ensures rebuilds when feature selections change.
- **SDK and image builder** make cross-building and custom images accessible.
- **`opkg`/`apk` output** and package indexes are handled by the build system.

**Worse than RPM**

- **Make macro soup.** `define`/`$(call)`/`$(eval)` is powerful but hard to read and debug.
- **Duplicated package definitions** and conditionals spread across the Makefile.
- **No hermetic per-package build**; it relies on the OpenWrt staging tree.
- **No lockfile.**
- **Little test/QA infrastructure** compared with RPM/Debian.

**Lesson:** a package definition should explicitly describe both the runtime package and the development/staging artifacts; Kconfig-style feature selection is a good UX.

Source: [OpenWrt package Makefile example](https://github.com/openwrt/packages/blob/master/net/curl/Makefile), [OpenWrt build docs](https://openwrt.org/docs/guide-developer/toolchain/building-packages).

---

### 11. Buildroot

**Better than RPM**

- **Small, approachable package recipe**: `FOO_VERSION`, `FOO_SOURCE`, `FOO_SITE`, `FOO_LICENSE`, `FOO_DEPENDENCIES`, `FOO_CONF_OPTS`, `FOO_MAKE_OPTS`, and install hooks.
- **Kconfig integration** makes configuration discoverable and dependency-aware.
- **Strong cross-compilation story**: internal or external toolchain, sysroot, staging/target separation.
- **`legal-info`** and **CycloneDX SBOM** generation are built in.
- **Hooks** at every step (`pre-configure`, `post-install`, etc.) are explicit.
- **No root required**, unlike many distro builders.

**Worse than RPM**

- **No binary package manager**; Buildroot builds a root filesystem, not installable packages. (Its FAQ says so explicitly.)
- **Coarse incremental rebuilds**; changing configuration often requires a full rebuild.
- **No dependency solver at the package level**; Kconfig selects, but there is no rich dependency model.
- **No scriptlets, file triggers, or package metadata** comparable to RPM.
- **Make + Kconfig**, not a general-purpose package format.

**Lesson:** cross-compilation and sysroot separation should be first-class; legal/SBOM output should be generated, not bolted on; simple recipes are possible even for complex builds.

Source: [Buildroot manual](https://buildroot.org/downloads/manual/manual.html).

---

### 12. Yocto / OpenEmbedded

**Better than RPM**

- **Task graph with explicit dependencies.** `do_fetch`, `do_unpack`, `do_patch`, `do_configure`, `do_compile`, `do_install`, `do_package`, `do_package_write_rpm`; tasks can declare inter-task dependencies.
- **Layers and `.bbappend`** are a mature composition model: base recipes, board layers, distro layers, local overrides.
- **`SRC_URI` plus per-file checksums** and `LIC_FILES_CHKSUM` make sources and licenses explicit.
- **`PACKAGECONFIG`** is a clean feature model with automatic dependency/option wiring.
- **`PACKAGES` and `FILES`** split output explicitly; `do_package` runs QA.
- **`BBCLASSEXTEND`** generates native/nativesdk/multilib variants from one recipe.
- **Shared-state (`sstate`) cache** gives fast incremental builds.
- **`insane` QA class** and **SPDX SBOM** generation.
- **`devtool` and `recipetool`** automate recipe creation and patching.

**Worse than RPM**

- **Huge complexity.** BitBake syntax, override syntax, layer ordering, and class interactions are a system of their own.
- **Slow evaluation and builds.**
- **Not a general-purpose distro package format**; it is an embedded image/build system.
- **Shell + Python + BitBake metadata** is a lot of languages.
- **Debugging failures can be very hard** because of the layers of indirection.
- **No clean separation between recipe and distro policy** in practice; layers and overrides blur it.

**Lesson:** task graphs, layers, feature flags (`PACKAGECONFIG`), explicit output splitting, sstate-style caching, QA and SBOM are all directly applicable.

Source: [Yocto recipe manual](https://docs.yoctoproject.org/dev/dev-manual/new-recipe.html), [tasks](https://docs.yoctoproject.org/ref-manual/tasks.html).

---

### 13. Nix

**Better than RPM**

- **Hermetic by construction.** Builds run in a sandbox with only declared inputs; network is disabled except for fixed-output derivations.
- **Content-addressed store.** Every dependency is an exact store path; no host libraries leak in.
- **Reproducibility is the default posture**, not an afterthought.
- **Binary caching** makes source builds practical.
- **Multiple outputs** (`out`, `dev`, `lib`, `doc`, `man`, ...) are first-class.
- **Cross-compilation** via `buildPackages`/`targetPackages` and `strictDeps`.
- **Overrides and overlays** are a clean composition model.
- **`nix-shell`/`nix develop`** gives the exact build environment.
- **Fixed-output derivations** make source fetching explicit and verifiable.

**Worse than RPM**

- **The Nix language is a barrier**; packaging is programming in a lazy functional language.
- **No FHS**; binaries need RPATH patching, wrappers and `makeWrapper`.
- **Rebuild propagation** is exact but can cause large rebuilds for small changes.
- **System integration is weak**: no scriptlets, no system users/groups in the RPM sense, no file triggers; NixOS modules handle system state separately.
- **Not a distro package format**; packages are store paths, not installable RPMs.
- **Binary cache trust and reproducibility of the cache** are operational concerns.

**Lesson:** hermetic sandboxing, exact inputs, multiple outputs, and a lock-like derivation are the gold standard; a distro format can adopt the *properties* without adopting the store.

Source: [Nix derivations](https://nix.dev/manual/nix/2.18/language/derivations), [Nixpkgs stdenv](https://nixos.org/manual/nixpkgs/stable/#chap-stdenv), [Nix Pills: stdenv](https://nixos.org/guides/nix-pills/19-fundamentals-of-stdenv.html).

---

### 14. GNU Guix

**Better than RPM**

- **Same functional/hermetic model as Nix**, with a coherent Scheme API.
- **G-expressions** make build code staged and explicit about dependencies.
- **`origin` records** make source provenance and hashes first-class.
- **`modify-phases`** is a clean way to adjust a build system's phases without rewriting them.
- **`guix lint`, `guix refresh`, `guix graph`, `guix size`, `guix challenge`** provide excellent packaging tooling.
- **`guix pack`** can emit tarballs, Docker images, etc.
- **`guix shell`** gives reproducible development environments.
- **Bootstrapping** is a first-class concern, with a transparent bootstrap chain.
- **Reproducibility checking** (`--rounds`, `guix challenge`) is built in.

**Worse than RPM**

- **Scheme is a barrier** for many packagers.
- **Smaller package ecosystem** and fewer enterprise integrations.
- **No FHS**, same as Nix.
- **System integration** is via Guix System services, not package scriptlets.
- **Not an RPM-producing distro format.**
- **Build times and store size** can be large.

**Lesson:** provenance, phase modification, linting, graph/size tooling and reproducibility challenges are excellent; a packaging format should ship these as commands, not as separate ecosystem projects.

Source: [Guix Defining Packages](https://guix.gnu.org/manual/1.5.0/en/html_node/Defining-Packages.html), [G-Expressions](https://guix.gnu.org/manual/stable/en/html_node/G_002dExpressions.html), [origin reference](https://guix.gnu.org/manual/1.5.0/en/html_node/origin-Reference.html).

---

### 15. Spack

**Better than RPM**

- **Specs and concretization** are a real solver model: abstract specs become exact graphs, with variants, compilers, versions and dependencies.
- **Variants are typed and constrained** (`variant(...)`, `depends_on(..., when="+feature")`, `conflicts(...)`).
- **Virtual packages and providers** are first-class.
- **Build-system classes** (`AutotoolsPackage`, `CMakePackage`, `MesonPackage`, `PythonPackage`, ...) are clean adapters.
- **`spack.lock`** captures the exact concretized graph for an environment.
- **Build caches and binary mirrors** make HPC source builds practical.
- **`spack test`** and **`spack audit`** add testing and linting.
- **Compiler-as-dependency** and ABI-aware `package_id` modelling are strong.
- **`spack develop`** supports working on a package in-place.
- **SBOM** support.

**Worse than RPM**

- **Python recipes are imperative**; arbitrary code can run during concretization/build.
- **Complexity is high**, especially for non-HPC users.
- **Not a distro package format**; installs into Spack's own tree, not `/usr`.
- **No system scriptlets/users/groups/file triggers.**
- **Source builds dominate**; binary packaging is a cache, not a distro contract.
- **Hermeticity is not as strong as Nix** by default.

**Lesson:** variant constraints, concretization, lockfiles, ABI-aware package identity, and build-system classes are all directly applicable.

Source: [Spack packaging guide](https://spack.readthedocs.io/en/latest/packaging_guide_creation.html), [spec syntax](https://spack.readthedocs.io/en/latest/spec_syntax.html), [environments](https://spack.readthedocs.io/en/latest/environments.html).

---

### 16. Conan

**Better than RPM**

- **Explicit binary model.** `settings`, `options`, `package_id`, `package_type`, `package_id_*_mode` define exactly when a rebuild is needed.
- **Build vs. host contexts.** `tool_requires` vs. `requires`, with separate profiles for build and host machines.
- **`test_requires`** keeps test-only dependencies out of the consumer graph.
- **Profiles and lockfiles** make builds reproducible across machines.
- **Recipe revisions** distinguish source changes from binary changes.
- **Generators** produce build-system integration (CMakeDeps, CMakeToolchain, etc.).
- **`conan graph`, `conan lock`, `conan test`** are solid tooling.
- **Binary remotes/caching** are core.

**Worse than RPM**

- **C/C++ only.** Not a general distro package format.
- **Python recipes** can execute arbitrary code.
- **No OS integration**: no users/groups, services, file triggers, scriptlets.
- **No FHS install**; packages live in the Conan cache.
- **No hermetic sandbox** by default.
- **No file manifest for system installation.**

**Lesson:** the binary compatibility model (`package_id` and rebuild modes) is the most sophisticated I found; a distro format should expose an explicit ABI/rebuild policy rather than leaving it to release bumps.

Source: [Conan conanfile attributes](https://docs.conan.io/2/reference/conanfile/attributes.html), [binary model](https://docs.conan.io/2/reference/binary_model.html).

---

### 17. vcpkg

**Better than RPM**

- **Clean split between metadata and build logic**: `vcpkg.json` declares name, version, dependencies, features, supports; `portfile.cmake` does the build.
- **Manifest mode** makes dependencies a project-level lock (with a baseline).
- **Triplets** are a first-class cross-compilation/configuration model.
- **Features** are declared with dependencies and platform constraints.
- **Binary caching** and ABI hashes make rebuilds fast and predictable.
- **Helper functions** (`vcpkg_cmake_configure`, `vcpkg_cmake_install`, `vcpkg_fixup_pkgconfig`, etc.) encode best practices.
- **Overlay ports** allow local customization without forking the registry.

**Worse than RPM**

- **CMake is the build language**; portfiles are imperative CMake scripts.
- **C/C++ only.**
- **No OS package integration.**
- **No strong sandbox**; builds run in a controlled tree but can access the host.
- **No system dependency solver**, no scriptlets, no file triggers.
- **No installed-package test model** comparable to Debian/RPM.

**Lesson:** metadata/build split, features, triplets and ABI hashes are good; CMake is not a good general recipe language.

Source: [vcpkg ports](https://learn.microsoft.com/en-us/vcpkg/concepts/ports), [vcpkg.json reference](https://learn.microsoft.com/en-us/vcpkg/reference/vcpkg-json).

---

### 18. Bazel

**Better than RPM**

- **Hermetic action graph.** Every action declares inputs and outputs; sandboxing is per-action; remote cache/execution is possible.
- **Explicit toolchains and platforms**, which makes cross-compilation principled.
- **Starlark is deterministic and analyzable** (no arbitrary I/O at analysis time).
- **Remote caching** gives fast, correct incremental builds.
- **`rules_pkg`** can produce RPMs, debs and tarballs from the same graph.
- **Aspect-oriented analysis** allows cross-cutting checks (SBOM, licenses, etc.).

**Worse than RPM**

- **Not a distro package format.** BUILD files describe build targets, not system packages.
- **Verbose and complex** for simple packages; rules can be a lot of code.
- **No system integration** (users, services, scriptlets, file triggers).
- **No package database or install/erase semantics.**
- **Steep learning curve** and a large ecosystem of rules with different quality.

**Lesson:** the action graph, explicit inputs/outputs, sandboxing and remote cache are the right execution model; a package builder should borrow them without exposing Starlark to packagers.

Source: [Bazel hermeticity](https://bazel.build/basics/hermeticity), [sandboxing](https://bazel.build/docs/sandboxing), [remote execution](https://bazel.build/remote/rules).

---

### 19. Homebrew

**Better than RPM**

- **Excellent UX and review culture.** `brew create`, `brew audit`, `brew style`, `brew test`, `brew livecheck`, `brew bump`.
- **`test do` is mandatory in practice** and tests the installed artifact, not just `make check`.
- **Bottles** make binary distribution first-class.
- **`depends_on` supports `:build`, `:test`, `:recommended`, `:optional`, `:no_linkage`**, plus OS/arch requirements.
- **`uses_from_macos`** elegantly handles system-provided dependencies.
- **`resource` blocks** pin auxiliary downloads with hashes.
- **`patch` blocks** support external, embedded and annotated patches with CVE/issue metadata.
- **`std_*_args`** centralizes build-system arguments.
- **`compatibility_version`** tracks ABI compatibility and dependent rebuilds.
- **`keg_only`, `caveats`, `service`** handle installation quirks.

**Worse than RPM**

- **Ruby is the definition language**; imperative and hard to analyze.
- **macOS/Linux-focused**, not a general distro format.
- **No real dependency solver**; it installs the latest formula and relies on revisions.
- **No FHS**; everything is under the Homebrew prefix.
- **No system package integration** (no scriptlets, no users/groups in the RPM sense).
- **No hermetic sandbox**; `superenv` sanitizes but does not isolate.
- **No subpackages** in the RPM sense.

**Lesson:** tests, audits, bottle/ABI metadata, patch annotations, and build-system argument helpers are all worth copying. A packaging format should make the "boring" quality checks automatic.

Source: [Homebrew Formula Cookbook](https://docs.brew.sh/Formula-Cookbook).

---

### 20. MacPorts

**Better than RPM**

- **Portgroups** are a mature composition mechanism (python, perl, cmake, meson, etc.).
- **Variants** are first-class and can depend on/conflict with each other.
- **Subports** allow multiple packages from one Portfile.
- **Phases are explicit** (fetch, extract, patch, configure, build, destroot, archive, install).
- **Trace mode** can detect undeclared dependencies and filesystem access.
- **`port lint`**, `livecheck`, and binary archives.
- **`rev-upgrade`** detects and repairs broken dependents after library changes.

**Worse than RPM**

- **Tcl is the language**; obscure and hard to tool.
- **macOS-only in practice.**
- **No hermetic sandbox by default.**
- **No rich/weak dependency model** comparable to RPM.
- **No lockfile.**
- **Small ecosystem and aging infrastructure.**

**Lesson:** portgroups, variants, subports, trace mode and `rev-upgrade` are useful ideas; Tcl is a cautionary tale about choosing an unfamiliar language for a packaging DSL.

Source: [MacPorts Guide](https://guide.macports.org/).

---

### 21. Flatpak

**Better than RPM**

- **Modules with nested dependencies** are easy to read and compose.
- **Sources are typed and hashed** (`archive`, `git`, `file`, `dir`, `script`, `patch`, `extra-data`).
- **Build-system adapters** (`autotools`, `cmake`, `cmake-ninja`, `meson`, `qmake`, `simple`) are clean.
- **`finish-args` makes sandbox permissions explicit** and reviewable.
- **Cleanup rules** are declarative and per-module or global.
- **Shared modules** provide reuse across apps.
- **Runtimes and SDKs** give a stable base and reduce duplication.
- **`flatpak-builder` sandbox** is a real isolation boundary.

**Worse than RPM**

- **Application-centric**, not a distro package format.
- **Bundles dependencies** instead of solving them at install time.
- **No system scriptlets/users/groups/file triggers.**
- **JSON/YAML schema is loose**, and the manifest can become a large imperative shell script.
- **No dependency solver**; modules are built in order.
- **No lockfile**; runtime/SDK versions are pinned by name, not content.
- **No file-level package metadata** for system installation.

**Lesson:** typed sources, modules, declarative cleanup, explicit sandbox permissions and runtime/SDK pinning are good; a distro format should keep dependency solving at install time.

Source: [Flatpak manifests](https://docs.flatpak.org/en/latest/manifests.html), [module sources](https://docs.flatpak.org/en/latest/module-sources.html).

---

### 22. Snap

**Better than RPM**

- **Parts with a five-step lifecycle** (pull, build, stage, prime, pack) and per-step overrides. The stage/prime distinction is useful: build-time files vs. runtime files.
- **Plugins** for common build systems and languages.
- **`build-packages` vs. `stage-packages`** separates build-time and runtime dependencies.
- **`organize` and file sets** make file placement/filtering declarative.
- **Confinement, plugs and slots** make security policy explicit.
- **`snapcraft lint`** and multiple linters (classic, library, metadata, GPU).
- **Extensions** reduce boilerplate (GNOME, KDE, etc.).
- **Remote build** and cross-build support.

**Worse than RPM**

- **Canonical-specific**; not a general distro package format.
- **No dependency solver**; `stage-packages` pulls from apt.
- **YAML plus shell overrides**; complex snaps become script-heavy.
- **No system package integration** in the RPM sense; confinement changes the model.
- **No lockfile**; bases and packages are name/version pinned.
- **Large, duplicated runtime payloads.**

**Lesson:** the stage/prime split, explicit build vs. runtime packages, typed lifecycle overrides, and linters are good ideas.

Source: [Snapcraft parts lifecycle](https://ubuntu.com/docs/snapcraft/9/explanation/parts-lifecycle/), [snapcraft.yaml reference](https://ubuntu.com/docs/snapcraft/9/reference/snapcraft-yaml/).

---

### 23. Slackware / SlackBuilds

**Better than RPM**

- **Maximum transparency.** A SlackBuild is a shell script; you can read every command. There is no macro layer and no hidden framework.
- **Very low tooling complexity.** `makepkg` creates a tarball; `installpkg`/`upgradepkg` install it.
- **`.info` file** records name, version, homepage, download, checksums, `REQUIRES`, maintainer.
- **`slack-desc`** is a simple, standardized description format.
- **SBo linting tools** (`sbolint`, `sbopkglint`).

**Worse than RPM**

- **No dependency metadata in the package manager.** `REQUIRES` is metadata for humans/tools; `installpkg` does not resolve dependencies.
- **No package database with dependency/version semantics** comparable to RPM.
- **No scriptlet/trigger model** beyond install scripts.
- **No sandbox, no lockfile, no reproducibility.**
- **No subpackages, no rich/weak deps, no file triggers.**
- **Manual everything.**

**Lesson:** transparency is valuable; a new format should keep the "read the whole recipe and know what will happen" property that macro-heavy specs lose.

Sources: [SlackBuilds submission guidelines](https://slackbuilds.org/guidelines/), [SlackBuild HOWTO](https://slackbuilds.org/howto/).

---

### 24. CRUX

**Better than RPM**

- **The `.footprint` file is a file manifest with expected contents.** `pkgmk` compares the built package against it and fails on mismatch. This is a simple, powerful regression check for file ownership.
- **`Pkgfile` is a small Bash script** with `name`, `version`, `release`, `source`, `build()`.
- **`prt-get` adds dependency resolution** on top of the dependency-free `pkgutils`.
- **`renames`** handles source filename collisions.
- **Rootless builds** via `rootlesskit` are documented.
- **Very small and understandable.**

**Worse than RPM**

- **Packages contain no metadata**; name/version are parsed from the filename.
- **Dependency handling is external and limited**; no versions in the package format.
- **No scriptlets/triggers, no subpackages, no rich/weak deps.**
- **No sandbox, no lockfile, no reproducibility.**
- **Tiny ecosystem.**

**Lesson:** the `.footprint` idea—compare the built tree to an expected manifest—is exactly the kind of guardrail RPM needs. A modern format should make the manifest explicit and fail closed.

Source: [CRUX package system](https://crux.nu/Main/Handbook3-8-Package).

---

### 25. Wolfi and melange

Wolfi is a deliberately minimal, rolling "undistro" designed for containers and embedded workloads. It is not a general-purpose desktop distribution. Packages are APKs built with **melange**; images are composed with **apko**. The package repository is curated, the package set tracks upstream security fixes aggressively, and the output is meant to be consumed as a package set or an OCI image, not as a traditional `/usr` install.

A melange build file is a single YAML document:

```yaml
package:
  name: hello
  version: 2.12
  epoch: 0
  description: "the GNU hello world program"
  copyright:
    - license: GPL-3.0-or-later
  dependencies:
    runtime: []

environment:
  contents:
    repositories:
      - https://packages.wolfi.dev/os
    keyring:
      - https://packages.wolfi.dev/os/wolfi-signing.rsa.pub
    packages:
      - build-base
      - busybox

pipeline:
  - uses: fetch
    with:
      uri: https://ftp.gnu.org/gnu/hello/hello-${{package.version}}.tar.gz
      expected-sha256: cf04af86dc085268c5f4470fbae49b18afbc221b78096aab842d934a76bad0ab
  - uses: autoconf/configure
  - uses: autoconf/make
  - uses: autoconf/make-install
  - uses: strip

subpackages:
  - name: hello-doc
    pipeline:
      - uses: split/manpages
    test:
      pipeline:
        - uses: test/docs

test:
  pipeline:
    - runs: |
        hello
        hello --version
```

**Better than RPM**

- **Pipeline-oriented and readable.** `uses:` names a reusable pipeline; `runs:` is the escape hatch. Common steps (`fetch`, `autoconf/configure`, `strip`, `split/manpages`) are named and shared.
- **Build environment is declared as packages plus repositories and keyrings.** `environment.contents` is explicit; there is no ambient host toolchain. Pipelines can declare `needs.packages` and even `needs.capabilities`.
- **Subpackages and per-subpackage tests are first-class.** A subpackage can have its own pipeline and its own test environment.
- **Matrix expansion over subpackages is real and in production.** `data:` declares a named axis; `subpackages: - range: <axis>` expands one subpackage template per row, with `${{range.key}}` and `${{range.value}}` available in names, dependencies, pipelines and tests. Wolfi uses this to build every Python library for 3.10–3.13:

  ```yaml
  data:
    - name: py-versions
      items:
        3.10: '310'
        3.11: '311'
        3.12: '312'
        3.13: '313'

  subpackages:
    - range: py-versions
      name: py${{range.key}}-${{vars.pypi-package}}
      pipeline:
        - uses: py/pip-build-install
          with:
            python: python${{range.key}}
      test:
        pipeline:
          - uses: python/import
            with:
              python: python${{range.key}}
              import: ${{vars.pypi-package}}
  ```

  This is a true matrix over *outputs*: each expanded subpackage gets its own name, runtime dependencies, build pipeline and tests. It is not a matrix over *build cells* — all rows share one build environment, one source checkout and one APK build, and there is no per-row build root or source package.
- **Templating is built in.** `${{package.*}}`, `${{subpkg.*}}`, `${{build.arch}}`, a user `vars` map, and regex-based `var-transforms` cover most string reuse.
- **Tests run in a fresh guest built with apko**, containing the package under test plus declared test dependencies. This is the installed-package test model done right.
- **Multi-architecture by default**, using QEMU/binfmt for foreign architectures rather than requiring a cross-compilation setup.
- **Containment with bubblewrap.** The build root is an apko-composed guest; commands run inside a bwrap container.
- **SBOM and signatures are normal outputs.** `melange` emits SBOMs; `apko` emits image SBOMs; the package repository is signed.
- **Update metadata is part of the recipe.** `[update]` and `melange bump` make version bumps and hash refresh a tool-assisted operation.
- **Options like `no-provides`, `no-depends`, `no-commands`, `no-versioned-shlib-deps`** give explicit control over generated dependencies.

**Worse than RPM**

- **YAML plus shell `runs:` is still a shell-based build system.** When a package leaves the happy path, the pipeline becomes a sequence of shell snippets. It is more structured than a spec, but not typed.
- **No lockfile.** `environment.contents` names packages and repositories, not exact resolved package hashes. Reproducibility depends on repository state and pinned versions.
- **The APK dependency model is simpler than RPM's.** No rich/weak dependency grammar comparable to RPM's, and no file-level trigger model as rich as RPM's.
- **No first-class per-file metadata manifest.** The pipeline installs into a destination tree; subpackage split pipelines move files. There is no `%config`/`%caps`/`%ghost` equivalent with RPM's depth.
- **Wolfi is curated and container-focused.** It is not a general-purpose distro, and its package set is intentionally narrow. That is a design choice, not a defect, but it limits how much can be generalized.
- **The matrix stops at subpackages.** `range:` expansion applies to `subpackages`, not to `pipeline`, `environment`, `package` identity or the build environment. The documented mechanism is one `range:` per subpackage entry: there is no documented Cartesian product across several axes, no `exclude`, no cell limit, and no per-cell build root or per-cell source package. Every row is built in the same environment, so rows cannot have disjoint toolchains.
- **`var-transforms` is a string-rewriting macro.** It creates variables with regex replacement. It is useful for version mangling, but it cannot express a build graph and it makes the template layer harder to reason about than a closed variable scope.
- **`epoch` means something different.** In Wolfi/melange, `epoch` is essentially the package rebuild counter (like RPM's `release`), not RPM's version-ordering epoch. The terminology is a trap for RPM packagers.
- **The pipeline library can hide behavior.** `uses: autoconf/make` is concise, but understanding it requires reading the pipeline definition; this is the same "macro by another name" risk.

**Lesson:** the pipeline model is the closest modern analogue to what RPM's `%build`/`%install` should have been: named, reusable, declarative steps with a quarantined `runs` escape hatch. The build environment as a declared package set, the fresh-guest tests, SBOM/signature output, and `data`/`range` matrix expansion are all directly applicable. The missing lockfile, the output-only matrix, and the lack of a typed per-file manifest are the gaps a new RPM format can fill.

Sources: [melange build file](https://github.com/chainguard-dev/melange/blob/main/docs/BUILD-FILE.md), [melange build process](https://github.com/chainguard-dev/melange/blob/main/docs/BUILD-PROCESS.md), [melange testing](https://github.com/chainguard-dev/melange/blob/main/docs/TESTING.md), [Wolfi repository](https://github.com/wolfi-dev/os).

---

### 26. apko

apko builds OCI images from APK packages using a YAML file. Its most important design decision is what it *does not* have: there is no `RUN` statement. An apko image is a declarative function of a package set, repositories, keyrings, accounts, paths, environment, entrypoint, architecture list and layering strategy.

```yaml
contents:
  repositories:
    - https://dl-cdn.alpinelinux.org/alpine/v3.22/main
  packages:
    - alpine-baselayout
    - nginx

entrypoint:
  command: /usr/sbin/nginx

accounts:
  groups:
    - groupname: nginx
      gid: 10000
  users:
    - username: nginx
      uid: 10000
  run-as: nginx

paths:
  - path: /run/nginx
    type: directory
    uid: 10000
    gid: 10000
    permissions: 0o755
```

**Better than RPM**

- **No arbitrary build steps.** This is the key to bitwise reproducibility: the image contents are a pure function of declared packages and configuration. There is no hidden `RUN` script that can download, mutate or drift.
- **Bitwise reproducible by default.** The README claims building twice produces exactly the same image; the design makes that plausible.
- **SBOMs are generated for the image**, covering the complete package contents.
- **Minimal/distroless by construction.** The image contains only the packages needed by the application.
- **Declarative accounts, paths, environment, entrypoint and layering.** These are typed, reviewable fields, not shell.
- **`include`** allows a base configuration to be layered, with explicit precedence.
- **Multi-architecture** (`archs`) and OCI annotations are first-class.
- **Fast** image builds; no package installation script execution.

**Worse than RPM**

- **It is not a package builder.** apko composes existing packages; it does not compile software.
- **The package repository is the source of truth.** Reproducibility depends on the repository retaining exact package versions; apko does not have a lockfile in the Nix sense.
- **No arbitrary configuration steps, by design.** If an image needs a generated file, it must come from a package or a `paths` mutation. That is a limitation for some workloads.
- **APK only.** Not RPM.
- **No system package lifecycle.** No scriptlets, no file triggers, no upgrade transactions in the image; image updates replace the image.

**Lesson:** the "no `RUN` in the composition layer" rule is powerful. It suggests a clean boundary for `rpmx`: packages are built by a rich, sandboxed build definition; images are composed by a separate, pure, declarative tool (apko-like, or bootc) from packages. It also reinforces that SBOMs and signatures should be outputs of the composition step, not afterthoughts.

Sources: [apko README](https://github.com/chainguard-dev/apko), [apko file format](https://github.com/chainguard-dev/apko/blob/main/docs/apko_file.md).

---

### 27. Chisel

Chisel cuts Debian/Ubuntu packages into **slices**: named subsets of a package's files with their own dependencies on other slices. A slice definition is YAML; the output is a minimal root filesystem containing only the selected slices.

**Better than RPM**

- **File-level minimalism without rebuilding packages.** You can take a subset of a `.deb` and its dependencies, rather than installing the whole package.
- **Slices have explicit dependencies on other slices**, so the minimal rootfs is still coherent.
- **The slice definitions are reviewable data**, not shell.
- **A manifest records exactly what was extracted**, which is useful for SBOMs and auditing.
- **It directly attacks the "package granularity is too coarse for containers" problem.**

**Worse than RPM**

- **Debian/Ubuntu only.**
- **It is not a package build format.** It slices existing packages; it does not define how to build them.
- **Slice maintenance is extra work**, and slices can drift when packages change.
- **No build isolation or reproducibility guarantee** beyond the input packages.
- **No RPM-like file metadata or scriptlets.**

**Lesson:** a package format can expose *file sets* as a first-class concept without splitting the package into more subpackages. RPM's `%files` is already a file set; Chisel suggests making those sets composable and independently consumable. For `rpmx`, this is an argument for output manifests that can be selected/sliced by image builders.

Source: [Chisel slices](https://ubuntu.com/chisel/docs/latest/explanation/slices/).

---

### 28. OSBuild, mkosi, bootc/rpm-ostree, BlueBuild and Rockcraft

These are image-first systems rather than package-definition formats, but they matter because they show where the RPM ecosystem is heading.

- **OSBuild** uses JSON manifests with stages (`org.osbuild.rpm`, `org.osbuild.files`, `org.osbuild.script`, `org.osbuild.selinux`, etc.) to build OS images. It is declarative and pipeline-oriented, and it is the engine behind RHEL Image Builder. It is powerful but low-level; a manifest is a graph of stages, not a package recipe.
- **mkosi** uses an INI configuration plus build/post-install scripts to assemble images from distro packages. It is pragmatic and flexible, but scripts are still the escape hatch and reproducibility depends on the package manager state.
- **bootc / rpm-ostree** treat the OS as a bootable OCI image. Packages are installed into an image, and updates replace the image. This is a clean separation of package build from OS composition, but the package definition is still a spec (or a Containerfile plus packages).
- **BlueBuild** layers a YAML recipe with modules (`rpm-ostree`, `dnf`, `copr`, scripts) on top of a base image. It is convenient for custom images, not a package format.
- **Rockcraft** uses a `rockcraft.yaml` with parts/plugins, much like Snapcraft, to produce OCI images. It is declarative at the top and shell-heavy at the edges.

**Better than RPM:** image composition is explicit, reproducible-ish, and separated from package builds; bootc/rpm-ostree make OS updates transactional and image-based; OSBuild stages are reusable and inspectable.

**Worse than RPM:** none of these define package-level dependencies, file ownership, scriptlets or an install transaction. They consume packages built by something else. They also tend to reintroduce shell at the image layer (mkosi, BlueBuild, Rockcraft), which is exactly what apko avoids.

**Lesson:** the future is a two-layer architecture: a package builder with rich semantics, and an image composer that is as declarative and reproducible as possible. `rpmx` should be the first layer and should expose a clean package-set/manifest interface so apko-like, OSBuild-like or bootc-like composers can consume it.

Sources: [OSBuild](https://osbuild.org/docs/), [mkosi](https://github.com/systemd/mkosi), [bootc](https://bootc.dev/), [rpm-ostree](https://coreos.github.io/rpm-ostree/), [BlueBuild](https://blue-build.org/), [Rockcraft](https://documentation.ubuntu.com/rockcraft/).

---

## What the landscape says, in aggregate

### Things RPM should keep

- File-level metadata and ownership semantics.
- Rich and weak dependencies.
- File triggers.
- Subpackages and multiple outputs.
- Debuginfo generation and build-id handling.
- Signed packages and a real package database.
- `%autosetup`-style patch automation.
- `BuildSystem`-style declarative adapters (but made typed).
- `%license`, `%doc`, `%config`, `%caps`, `%ghost`, `%attr`, `%verify`.
- `%generate_buildrequires`-style dynamic dependency discovery (but resolved before the build, not via a second SRPM).

### Things RPM should steal

| Idea | Source | Why |
|---|---|---|
| Metadata / build logic / file-list separation | Debian | Each artifact has one job; easier to review and tool. |
| Named pipeline with override/hook points | Debian `dh` | Change one step without rewriting the phase. |
| Build profiles | Debian | Vary a build without editing the recipe. |
| Installed-package tests | Debian autopkgtest, Homebrew | `make check` is not enough; test the installed artifact. |
| Typed feature constraints | Gentoo `REQUIRED_USE`, Spack variants | Catch invalid combinations before building. |
| Dependency scopes (`DEPEND`/`BDEPEND`/`RDEPEND`/`IDEPEND`) | Gentoo, Void, Yocto | Build machine vs. target vs. runtime vs. install time. |
| Slots/subslots | Gentoo | Parallel-installable versions and ABI rebuild triggers. |
| Build-style adapters | Void, Spack, Flatpak, Yocto | Encode common build systems once. |
| Standard split helpers | Alpine, Void, Solus | `-dev`, `-doc`, `-libs`, `-static`, `-dbg` should be policy, not copy-paste. |
| Explicit file manifest | FreeBSD `pkg-plist`, CRUX `.footprint`, Debian `*.install` | Total ownership; catch under/over-capture. |
| `print-PLIST` / manifest generation | pkgsrc, FreeBSD | Generate a candidate manifest, then review it. |
| Buildlink-style isolation | pkgsrc | Prevent undeclared host dependencies. |
| Layers / `.bbappend` / overlays | Yocto, Nix | Compose distro policy without forking recipes. |
| Hermetic sandbox + fixed-output sources | Nix, Guix | Correct caching, reproducibility, auditability. |
| Content-addressed lock | Nix, Guix, Spack, Conan | Exact inputs; `--locked` means something. |
| ABI-aware package identity | Conan, Gentoo subslots | Know when dependents must rebuild. |
| Action graph / remote cache | Bazel | Fast, correct incremental builds. |
| Explicit sandbox permissions | Flatpak, Snap | Security policy is reviewable. |
| Patch annotations (CVE, upstream issue) | Homebrew | Makes patch debt visible. |
| Linters as first-class commands | Homebrew, pkgsrc, FreeBSD, Solus | Quality is enforced, not hoped for. |
| SBOM / provenance generation | Yocto, Buildroot, Spack, Nix | Supply-chain evidence is an output, not a side project. |
| `reverts` metadata | Void | Handle bad upgrades explicitly. |
| Alternatives metadata | Void, Debian | Common system integration without scriptlets. |
| Named reusable pipeline steps | melange | Common build steps become a shared, reviewable library instead of copied shell. |
| Build environment as a declared package set | melange | Explicit repositories, keys and build packages; no ambient host toolchain. |
| Fresh-guest installed-package tests | melange, Debian autopkgtest | Test the actual package in a minimal guest, not just the build tree. |
| No arbitrary `RUN` in image composition | apko | Bitwise reproducibility and complete SBOMs become possible. |
| File-level package slices | Chisel | Minimal images from existing packages without rebuilding them. |
| Package-set/manifest interface for image builders | apko, OSBuild, bootc | Clean boundary between package build and image composition. |
| SBOM and signatures as normal outputs | melange, apko, Yocto, Buildroot | Supply-chain evidence ships with the artifact. |
| Update metadata plus a bump command | melange | Version bumps and hash refreshes are tool-assisted and reviewable. |
| Matrix expansion of one package template into many outputs | melange `data`/`range` (Wolfi Python packages) | Proves that per-version output families belong in the recipe, not in `%if` or duplicated files. |

### What the modern undistro model validates

Wolfi/melange/apko is the strongest recent evidence for the thesis of this document:

1. **Package build and image composition should be separate layers.** melange builds packages; apko composes images from packages. Each layer has its own language and its own invariants.
2. **The composition layer should not run arbitrary commands.** apko's lack of `RUN` is not a limitation; it is what makes bitwise reproducibility and complete SBOMs credible.
3. **The build environment should be a declared package set.** `environment.contents` with repositories, keyrings and packages is much closer to a lock than a host chroot with implicit tools.
4. **Tests should run against the installed package in a fresh guest.** melange's test model is a direct improvement over an optional `%check` that runs in the build tree.
5. **Evidence should be an output, not a separate project.** SBOMs and signatures are part of the build result.
6. **A matrix belongs in the recipe, not in `%if` or duplicated files.** Wolfi's `data`/`range` expansion builds every Python library for four interpreters from one file, with per-version names, dependencies, pipelines and tests. It is the clearest proof that matrix output families are a data-model problem.

It also shows the limits:

- **YAML plus `runs:` shell is still shell.** Pipeline reuse helps, but the escape hatch is where complexity hides.
- **There is no lockfile.** The build environment is a package list, not a content-addressed closure.
- **The package dependency model is simpler than RPM's.** APK does not have RPM's rich/weak dependency grammar or file-trigger depth.
- **There is no RPM-style per-file metadata manifest.** The destination tree and split pipelines replace `%files`, but not `%config`/`%caps`/`%ghost` semantics.
- **The matrix is output-shaped, not cell-shaped.** `range:` expands subpackages inside one build environment and one source package. There is no per-row build root, toolchain, source package or cross-axis product. [`11-matrices.md`](11-matrices.md) generalizes it to build cells.

The design in [`03-design.md`](03-design.md) takes the first six lessons and adds the lock, typed manifest and RPM semantics that melange/apko lack.


### Things RPM should avoid

- Turing-complete macro expansion at parse time.
- `%if` on distro macros inside package recipes.
- One file that mixes metadata, build logic, file lists and policy.
- Implicit host dependency discovery ("automagic").
- Unstructured shell as the *only* extension mechanism.
- File capture by glob without a totality check.
- Network access during the build phase.
- Scriptlets as the primary way to integrate with the system.
- Untracked source downloads; no hash, no lock.
- Auto-generated changelogs that depend on mutable git state at build time (resolve them into the lock instead).
- Unbounded variant matrices without constraints or test coverage.

---

## References

Primary documentation used for this survey:

- RPM: [spec format](https://rpm.org/docs/latest/manual/spec.html), [declarative builds](https://rpm.org/docs/latest/manual/buildsystem.html), [dependencies](https://rpm.org/docs/6.1.x/manual/dependencies.html), [boolean dependencies](https://rpm.org/docs/latest/manual/boolean_dependencies.html), [autosetup](https://rpm.org/docs/latest/manual/autosetup.html), [timeline](https://rpm.org/timeline), [rpmbuild(8)](https://man7.org/linux/man-pages/man8/rpmbuild.8.html)
- Debian: [Policy](https://www.debian.org/doc/debian-policy/), [relationships](https://www.debian.org/doc/debian-policy/ch-relationships.html), [control fields](https://www.debian.org/doc/debian-policy/ch-controlfields.html), [dh(1)](https://manpages.debian.org/unstable/debhelper/dh.1.en.html), [maint-guide](https://www.debian.org/doc/manuals/maint-guide/), [copyright format](https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/)
- Arch: [PKGBUILD(5)](https://pacman.archlinux.page/PKGBUILD.5.html), [makepkg(8)](https://man.archlinux.org/man/makepkg.8), [ArchWiki PKGBUILD](https://wiki.archlinux.org/title/PKGBUILD)
- Alpine: [APKBUILD(5)](https://github.com/alpinelinux/abuild/blob/master/APKBUILD.5.scd), [APKBUILD reference](https://wiki.alpinelinux.org/wiki/APKBUILD_Reference)
- Gentoo: [DevManual](https://devmanual.gentoo.org/), [variables](https://devmanual.gentoo.org/ebuild-writing/variables/), [USE flags](https://devmanual.gentoo.org/general-concepts/use-flags/), [phase functions](https://devmanual.gentoo.org/ebuild-writing/functions/), [eclasses](https://devmanual.gentoo.org/ebuild-writing/using-eclasses/)
- Void: [xbps-src manual](https://github.com/void-linux/void-packages/blob/master/Manual.md)
- Solus: [package.yml](https://help.getsol.us/docs/packaging/package.yml)
- FreeBSD: [Porter's Handbook](https://docs.freebsd.org/en/books/porters-handbook/book/)
- pkgsrc: [pkgsrc guide](https://www.netbsd.org/docs/pkgsrc/creating.html)
- OpenWrt: [package docs](https://openwrt.org/docs/guide-developer/toolchain/building-packages), [curl example](https://github.com/openwrt/packages/blob/master/net/curl/Makefile)
- Buildroot: [manual](https://buildroot.org/downloads/manual/manual.html)
- Yocto: [new recipe](https://docs.yoctoproject.org/dev/dev-manual/new-recipe.html), [tasks](https://docs.yoctoproject.org/ref-manual/tasks.html), [BitBake manual](https://docs.yoctoproject.org/bitbake/singleindex.html)
- Nix: [derivations](https://nix.dev/manual/nix/2.18/language/derivations), [stdenv](https://nixos.org/manual/nixpkgs/stable/#chap-stdenv), [Nix Pills](https://nixos.org/guides/nix-pills/19-fundamentals-of-stdenv.html), [fixed-output derivations](https://nix.dev/manual/nix/2.24/language/advanced-attributes.html)
- Guix: [Defining Packages](https://guix.gnu.org/manual/1.5.0/en/html_node/Defining-Packages.html), [G-Expressions](https://guix.gnu.org/manual/stable/en/html_node/G_002dExpressions.html), [origin reference](https://guix.gnu.org/manual/1.5.0/en/html_node/origin-Reference.html)
- Spack: [packaging guide](https://spack.readthedocs.io/en/latest/packaging_guide_creation.html), [spec syntax](https://spack.readthedocs.io/en/latest/spec_syntax.html), [environments](https://spack.readthedocs.io/en/latest/environments.html)
- Conan: [conanfile attributes](https://docs.conan.io/2/reference/conanfile/attributes.html), [binary model](https://docs.conan.io/2/reference/binary_model.html)
- vcpkg: [ports](https://learn.microsoft.com/en-us/vcpkg/concepts/ports), [vcpkg.json](https://learn.microsoft.com/en-us/vcpkg/reference/vcpkg-json)
- Bazel: [hermeticity](https://bazel.build/basics/hermeticity), [sandboxing](https://bazel.build/docs/sandboxing)
- Homebrew: [Formula Cookbook](https://docs.brew.sh/Formula-Cookbook), [Formula API](https://docs.brew.sh/rubydoc/Formula.html)
- MacPorts: [Guide](https://guide.macports.org/)
- Flatpak: [manifests](https://docs.flatpak.org/en/latest/manifests.html), [module sources](https://docs.flatpak.org/en/latest/module-sources.html)
- Snap: [parts lifecycle](https://ubuntu.com/docs/snapcraft/9/explanation/parts-lifecycle/), [snapcraft.yaml](https://ubuntu.com/docs/snapcraft/9/reference/snapcraft-yaml/)
- Slackware: [SlackBuilds guidelines](https://slackbuilds.org/guidelines/), [HOWTO](https://slackbuilds.org/howto/)
- CRUX: [package system](https://crux.nu/Main/Handbook3-8-Package)
- Wolfi: [repository and overview](https://github.com/wolfi-dev/os), [contributing](https://github.com/wolfi-dev/os/blob/main/CONTRIBUTING.md)
- melange: [README](https://github.com/chainguard-dev/melange), [build file reference](https://github.com/chainguard-dev/melange/blob/main/docs/BUILD-FILE.md), [build process](https://github.com/chainguard-dev/melange/blob/main/docs/BUILD-PROCESS.md), [testing](https://github.com/chainguard-dev/melange/blob/main/docs/TESTING.md)
- apko: [README](https://github.com/chainguard-dev/apko), [file format](https://github.com/chainguard-dev/apko/blob/main/docs/apko_file.md)
- Chisel: [slices](https://ubuntu.com/chisel/docs/latest/explanation/slices/), [slice definitions](https://ubuntu.com/chisel/docs/latest/reference/chisel-releases/slice-definitions/)
- OSBuild: [documentation](https://osbuild.org/docs/)
- mkosi: [repository](https://github.com/systemd/mkosi)
- bootc: [documentation](https://bootc.dev/)
- rpm-ostree: [documentation](https://coreos.github.io/rpm-ostree/)
- BlueBuild: [documentation](https://blue-build.org/)
- Rockcraft: [documentation](https://documentation.ubuntu.com/rockcraft/)
