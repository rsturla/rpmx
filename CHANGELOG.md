# Changelog

All notable changes to `rpmx` are recorded here. The project is pre-1.0 and
does not promise a stable schema or CLI yet.

## Unreleased

- `generators.perl` accepts a mapping as well as a bool: `perl: { provides:
  false }` or `perl: { requires: false }` keeps one side of the generated
  `perl(...)` dependencies. A package that ships the interpreter needs its
  module provides but not the requires its own files record, which name other
  systems' helper modules and optional back ends that nothing provides.

- **Breaking:** `@dev` no longer expands to `/usr/lib*/lib*.a`. Static archives
  are only selected by `@static`, so a development output cannot silently
  absorb a static-library output. Recipes that relied on `@dev` for static
  archives must add `@static` to the output that owns them.

- Package lint now blocks the output conventions that used to be review-only:
  `-dev` and `-common` output names, static archives outside `-static`
  (except compiler runtime archives in the compiler's main output), headers or
  unversioned `.so` links in `-static`, `-static` without a matching `-devel`
  dependency, `-libs` depending on its command package, and a command package
  that does not depend on its `-libs` output. Private helpers directly under
  `/usr/libexec` are also an error.

- **Breaking:** profiles no longer accept `flags` and recipes no longer accept
  `build.flags`. Distribution compiler and linker policy is package-owned:
  install the package that supplies the compiler specs or wrapper scripts in
  the build root. `cross.cflags`, `cross.cxxflags` and `cross.ldflags` remain
  for cross-only toolchain quirks.

- `source-date-epoch` is no longer required in every recipe. When it is omitted,
  rpmx uses the newest mtime from the source archives, or the pinned git commit
  time, and records that value in the SRPM and lock. `rpmx-defaults.yaml` can
  supply a repository fallback for packages with no upstream source. An explicit
  recipe value or `--source-date-epoch` still wins.
- The autotools, Make, CMake and Meson steps accept `dir` and pass their full
  argument list through to configure/build/install. `autotools/configure`
  supports out-of-tree builds and derives `--host`/`--build` in a cross build.
  Build and check steps use the sandbox `$JOBS` instead of a hardcoded `-j1`.
- **Breaking:** a step is named after the command it runs, so a command has
  exactly one step. `autotools/build`, `autotools/check` and `autotools/install`
  ran the same `make` commands as `make/build`, `make/check` and `make/install`
  and are removed; replace them with the `make` steps and keep
  `autotools/configure` in front. `autotools/check` had no callers. CMake and
  Meson keep their own build, check and install steps because those tools drive
  the build themselves.
- Added `license/copy`, which installs per-output license copies under
  `$PREFIX/share/licenses/<output>/` without repeating a shell loop.
- Added `test/hardening-check`, which walks the staged ELF files and checks
  PIE, NX, RELRO, BIND_NOW, stack protector, fortified libc, the target's
  CFI/branch-protection property and build IDs. `paths` narrows the scan and
  `skip` disables individual checks where the package cannot satisfy them.
- Added `test/compiler-hardening-check`, which compiles a probe with the
  configured compiler wrapper, checks the resulting executable's hardening
  properties, and verifies that a shared-library link still succeeds. It is
  meant for compiler-options packages such as `openssf-compiler-options`.
- A profile's local `file://` repository and key paths may be relative to the
  profile file. `load_profile` anchors them at the profile directory, so a
  checked-out bootstrap repository works without an absolute CI path.

- Source fetches use separate resolve, connect, response, read and body
  timeouts instead of one 60-second request budget. Transport failures retry
  three times with backoff; digest mismatches do not. Multiple `https`
  locators for the same digest are tried in order.

- The first output is now the main package and receives staged files that no
  other output claims, matching Wolfi/melange. There is no second ownership
  mode: strictness is lint policy, not recipe configuration.
  Outputs can use `@dev`, `@doc`, `@man`, `@info`, `@locales`, `@static`,
  `@pkgconfig`, `@headers` and `@debug` file presets instead of enumerating
  every installed path. Build output reports each output's file count and
  payload size, and package lint warns when the main package collects
  development or documentation payload and suggests the matching preset.
  `rpmx explain files` shows which output is the main package.

- Tests can be declared under an output. An installed-stage output test
  prepares a root with that output and the sibling outputs that provide its
  runtime dependencies, instead of installing every output. Recipe-level tests
  still install all outputs. Use `--test OUTPUT/NAME` when several outputs
  reuse a test name.

- Installed-stage test roots now resolve the strong runtime requirements of the
  selected built RPMs from the same repositories, instead of requiring every
  external runtime package in `build.dependencies.tests`. Requirements provided
  by another selected output are installed from the build output and are not
  resolved from a repository. Explicit test packages are still needed for tools
  or data that are not runtime requirements. Only missing package-level
  requirements are installed; library soname requirements are left to the
  seeded root, because resolving a soname and a package name separately can
  select two conflicting providers for the same library.

- Reading built RPM metadata treats a missing `RPMTAG_EPOCH` as epoch 0. rpm
  omits the tag for packages without an epoch, so test roots no longer fail
  before installing them.

- Tests accept a Wolfi/melange-style environment:
  `environment.contents.packages` lists packages installed only for that test's
  disposable root, and `environment.environment` sets test-only variables.
  Tests with different package sets get separate roots; the package-wide
  `build.dependencies.tests` still applies to every installed test. Test
  packages are rejected on `stage: build`, where they would be silently
  ignored.

- Recipes can inherit `package.maintainers` from `rpmx-defaults.yaml` in the
  recipe directory or a repository ancestor. The recipe wins when it declares
  its own maintainers. The defaults file joins the recipe bundle, so the lock
  and SRPM record it and a rebuild remains self-contained.

- Bootstrap fixed-point passes hide every provider except the stage output,
  including the seed repositories. A package no longer needs an inflated
  release or epoch to outrank Fedora while the seed is still in the profile.

- `rpmx bootstrap plan` validates a dedicated `rpmx-bootstrap` manifest and
  prints an ordered stage graph without resolving roots. Stages select recipes
  by discovery or explicit globs, reject duplicate/multi-cell recipes, require
  explicit packages on seed and cross stages, reject dependency cycles unless
  every member is in `fixed-point`, and choose a deterministic
  `source-date-epoch`. The plan prints seed/generation provenance, root/lock/repo
  paths, the effective timestamp and a `next:` command; `--json` exposes the
  same data. `RPMX-BOOTSTRAP-*` diagnostics have stable exit codes.

- Bootstrap manifest paths resolve relative to the manifest file, including
  parent components such as `../packages`, so a manifest can live in
  `.bootstrap/` without absolute paths. Native stages assemble a normal dnf
  root (with scriptlets and an RPM database); only cross sysroots use the
  payload-only extractor. Multi-arch plans keep each architecture's roots,
  fixed-point candidates and locks separate.

- Bootstrap stages can publish a read-only assembled root. The publisher adds
  owner write permission only for the directory rename and restores the
  original mode afterwards; stale read-only backups are removed through the
  namespace-aware root remover. This makes repeated `rpmx bootstrap build`
  runs work with `--force` instead of failing with `Permission denied`.

- Package SBOMs no longer list `source-root.lock.json` as a source. It records
  the root used for source assembly, so treating it as a package input made
  otherwise identical rebuilds produce different SBOMs and prevented a
  bootstrap fixed point from converging.

- Root digests read mode-000 files without requiring namespace privileges.
  `digest_root` first tries the tree as-is, then temporarily grants the owner
  read access and restores every mode, and only then falls back to
  re-executing under a user namespace. Fedora's `/etc/gshadow` previously made
  `rpmx bootstrap build` fail with `RPMX-ROOT-014` on a fresh candidate root.

- Bootstrap stages accept a `replaces` map from an Eternal output package name
  to one or more seed package names. This lets the ladder keep a stable output
  name while hiding a differently named seed provider; a fixed-point pass
  applies the same mapping to every other repository. The recipe still owns
  the matching `provides`.

- Bundled components are discovered from vendor metadata and emitted as
  `bundled(...)` provides. Go reads `vendor/modules.txt`, Cargo reads
  `Cargo.lock` when a `vendor/` tree exists, and Node projects read
  `package-lock.json`, `yarn.lock` (classic and Berry) or `pnpm-lock.yaml`
  plus their installed tree/cache. Node catalogers follow the production
  dependency closure, including workspace importers, and do not advertise
  dev-only packages. Python reads `.dist-info`/`.egg-info`
  metadata and `vendored.txt`, Ruby reads `Gemfile.lock` and vendored
  gemspecs, and PHP reads `composer.lock` or `vendor/composer/installed.json`.
  Locks without a vendor tree or installed payload are warnings, not provides.
  `rpmx explain components` shows the inventory, evidence, scope and final
  provides per output; `--source` inspects an assembled source workspace.
  C and C++ declare components with `outputs[].bundled`, the
  `bundled/declare` step, or a build-time `$RPMX_BUNDLED_DIR/<output>/*.list`;
  the engine creates the per-output
  directories and rejects conflicting versions. The inventory is embedded in
  the SRPM as `RPMX-BUNDLED.json` and covered by `RPMX-MANIFEST.json`.
- Debug information is opt-in per recipe, as in melange: `uses: debuginfo`
  replaces `strip`, keeps debug sections under `$PREFIX/lib/debug`, and the
  recipe declares the output that claims them. The step fails when no staged
  ELF contains DWARF, so a missing `-g` cannot ship an empty package. The
  `debuginfo` profile field, the automatic `-debuginfo` subpackage and the
  sandbox's `-g`/`RPMX_DEBUGINFO` injection are gone; a recipe sets `-g` in
  `build.environment`, and its flags now extend the cross toolchain instead of
  replacing it.
- `rpmx build` and `rpmx rebuild` run the recipe's tests by default, so a green
  build means tested. `--no-tests` opts out and `--skip-installed` runs only
  build-stage tests; `--run-tests` remains as an explicit no-op.
- A test root that cannot install the built RPMs only because the package
  under test conflicts with something already installed (for example a
  distribution default implementation) is retried without dependency
  checking, with a warning naming the replacement. A missing requirement still
  fails.
- Cross-compilation: a profile with a `cross` block builds a target
  architecture with a host-native toolchain against a target sysroot, with no
  emulation. `packages.install` is the sysroot; `cross.packages` is the build
  root; `needs.tools` and `needs.host` route to the right one. The sandbox
  exports the triple, `CC`/`CXX`/`AR`/`LD`/`STRIP`/`OBJCOPY`, sysroot-aware
  pkg-config, derived `CGO_*` flags and generated CMake/Meson toolchain files.
  Two checks catch a wrong build: every staged ELF must match the target
  machine, and every `DT_NEEDED` must exist in the sysroot.
- `cross.bootstrap: true` enables a native bootstrap: the seed build root and
  the target sysroot may share an architecture while staying separate roots.
  Without an explicit `cross.triple` the build uses the seed's native command
  names (`gcc`, `g++`, ...), and the SRPM lock records that choice so
  `rpmx rebuild` recreates the same toolchain. The build-root sysroot mount
  point is created before the root digest, so a cached root no longer fails
  under a read-only sandbox.
- `rpmx bootstrap rebuild` rebuilds a stage package against an explicit stage
  root lock and re-emits its SRPM. The replacement SRPM embeds the new lock and
  records the old root/sysroot digests under `replaced`; the input SRPM is
  never modified and `--srpm-out` must name a different directory. Plain
  `rpmx rebuild` keeps rejecting a mismatched lock with `RPMX-ROOT-016`.

- `rpmx bootstrap build` supports fixed-point stages: input repositories are
  filtered with `exclude`, the stage output becomes the winning repository, and
  only the `fixed-point` recipes are rebuilt until the RPM bytes and root
  digests match. A non-converging stage fails with `RPMX-BOOTSTRAP-005` and
  names the changed packages and last two root digests. `max-iterations`
  defaults to 3.

- `rpmx bootstrap build` resolves one build-root/sysroot pair per stage and
  runs the stage recipes in dependency order. It synthesizes the stage profile
  in memory (input stage repositories, recipe-required packages, `extra-tools`
  and `extra-sysroot`), writes `<roots>/<stage>/<arch>/root` and
  `<locks>/<stage>/<arch>.lock.yaml`, records seed/generation/requirement
  provenance,
  indexes the stage output with the manifest or stage `source-date-epoch`, and
  publishes it atomically. A non-empty output or root needs `--force`;
  `--locked` verifies stage locks and fails with `RPMX-BOOTSTRAP-006` on
  missing or drifted state; `--offline` names the stage and online command on a
  cache miss.
- A cross SRPM embeds its sysroot lock and toolchain, so `rpmx rebuild` needs
  no profile. `rpmx lock` records both the build root and the sysroot, and
  `--locked` reports `changed: sysroot` on drift. Installed-stage tests run in
  a target test root; `--skip-installed` runs only build-stage tests.
- `rpmx shell` exports the same environment a build step sees.
- `rpmx build --save-test-root DIR` keeps a prepared installed-stage test root
  (with the package under test) and writes its lock beside it;
  `rpmx test --test-root DIR` runs against it with no build root, profile,
  cache or network. `rpmx doctor --recipe` reports whether both roots are
  cached.
- The `DT_NEEDED` link-closure check runs for every assembled root, so native
  and emulated builds catch a missing library too.
- `go/build` and `go/test` accept `cgo: false`; the default stays on for a
  distribution package.
- The pre-build dependency check trusts an automatic root's resolution, so a
  declared dependency that resolves to a virtual provider (`zlib-devel` ->
  `zlib-ng-compat-devel`) no longer fails.
- A foreign target without a cross profile warns that steps are emulated, and
  `--unsafe-host-root` refuses a cross build.

- Foreign builds default to bubblewrap + host binfmt. `auto` and `--runner
  bwrap` run native steps directly and foreign steps through an enabled
  `F`-flag qemu-user handler; `--runner qemu` remains the explicit full-VM
  path. A missing handler fails before any step with `RPMX-RUNNER-002`.
- The build-root requirement is runner-independent. A QEMU appliance is a
  sidecar `appliance.lock.yaml` beside the cached root, not a `runner-assets`
  section in `root.lock.yaml`; `--locked` and `--runner qemu` share the same
  root and cannot drift because of the runner.
- Profiles may list several target architectures as an allowed set;
  `Profile::for_target` derives one root per target. `rpmx root resolve` gains
  `--arch`.
- Human progress output is now a leveled `INFO`/`WARN`/`ERRO` log on stderr,
  with a plan header, a resolved root block (builder and repositories), numbered
  steps, per-phase durations, relative artifact paths and a final artifact
  count. Every step line and guest line carries the active stage (`source`,
  `build`, `test`). Guest stdout and stderr are prefixed and written to stderr
  too, so stdout is only the artifact list or the single JSON document.
  `--verbose` lists the resolved root package set and step durations, and
  `--color` controls ANSI color.
- QEMU builds no longer boot the build root. A separate, locked transport
  appliance is resolved from the profile repositories, assembled into its own
  cache, and attached as a second disk. Build steps run on a chrooted build
  channel while an unchrooted control channel mounts the disk and moves the
  workspace and stage. Profiles no longer list kernel, dracut, networkd or SSH
  packages; the build root keeps its own closure. The appliance lock is a
  sidecar beside the cached root and installed test roots no longer install the
  boot set. `rpmx runner resolve|assemble|inspect` handles appliances
  independently (`rpmx appliance` remains an alias), and `--runner-asset-root`
  lets `build`, `source`, `test` and `rebuild` use a pre-assembled asset.
  `rpmx doctor` reports all runner tools and cached assets through the runner
  registry.
- Steps no longer hardcode guest paths. Built-in steps use `$STAGE`, `$PREFIX`,
  `$LIBDIR` and `$JOBS` (or the `${...}` placeholders); the planner keeps those
  placeholders in the plan, lock and SRPM, and the loader rejects a literal
  guest path with `RPMX-STEP-019`. Profiles gain `paths.prefix` and
  `paths.libdir`, recorded in the root lock.
- File-payload dependency generators: ELF sonames, symbol versions and `.so`
  symlink requires; pkgconfig provides and requires; shebang interpreters;
  Python ABI and `pythonX.Ydist(...)`; Perl modules; desktop/AppStream
  provides; and debuginfo build-ids.
- Per-output `generators:` switches and exact-match `exclude` lists.
- Native SRPM and binary RPM writers, with `rpmbuild` never invoked.
- Weak runtime dependencies (`recommends`, `suggests`, `supplements`,
  `enhances`) in recipes, RPM headers and repository metadata.
- Locked build roots resolved with dnf5 and assembled offline from the CAS.
- Source snapshots, patches, git sources and a content-addressed cache.
- A recipe `context:` directory copied into the source workspace, with one tree
  digest in the lock and inclusion in the source snapshot and SRPM.
- Matrix and template planning, reusable steps, build and installed tests.
- SPDX 2.3 SBOMs, CPE references and optional OpenPGP signing.
- File directories, per-path owner/group/mode and file capabilities.
- `rpmx repo` repository metadata and signed `repomd.xml`.
- One-command `rpmx build` from recipe to SRPM to binary RPMs.
- Host/target architecture separation and native-architecture defaults.
- A session-based `Runner` interface with bubblewrap, podman and qemu-system
  backends. Foreign builds use a full VM with a host-only SSH control NIC and
  tar-based workspace transfer; root assembly and test-root installation use
  podman --arch. Root assembly, SRPM embedding and installed tests follow the
  same host/target contract.
- QEMU workspace transfers stream over ssh instead of buffering whole archives;
  step output streams live, guest stderr retains only a bounded tail, and the
  serial log is capped.
- Untrusted decompressed payloads stream to disk instead of host memory, with
  no arbitrary SRPM/RPM or download size ceiling. Structural limits cover tar
  extension headers, sparse entries, bundle metadata and decompressed kernel
  images; guest sync-out keeps an 8 GiB cap because it is attacker output.
- `--jobs` and `--step-timeout` require a positive value; a step timeout can no
  longer be disabled.
