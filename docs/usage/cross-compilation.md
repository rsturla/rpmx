# Cross-compilation

rpmx builds for another architecture with a native toolchain. The compiler,
sandbox and every step run on the host; the output is for the target. Nothing
is emulated, so a foreign build costs no more than a native one.

This is different from building a foreign root with binfmt, where the target's
own compiler runs under emulation. Use binfmt when a recipe genuinely has to
execute target code; use cross when it does not.

## Declaring a cross profile

A profile becomes a cross profile when it has a `cross` block. The two package
lists describe two different roots:

```yaml
document: rpmx-profile
version: 1
name: my-distro-aarch64
releasever: "44"
archs: [aarch64]            # the target
repositories:
  - id: base
    baseurl: https://example.invalid/fedora
packages:
  install: [libc-devel, zlib-devel]     # the sysroot: headers and libraries
cross:
  packages: [bash, coreutils, make, gcc-aarch64-linux-gnu]  # the build root
  sysroot_packages: []                  # extra headers the build needs
```

- `packages.install` is the **sysroot**: target packages whose files the host
  compiler reads. It is unpacked from the locked RPMs, never installed by a
  guest `dnf`, so it needs no binfmt handler and no target execution.
- `cross.packages` is the **build root**: host packages, including the
  toolchain. Steps run here.
- `cross.sysroot_packages` appends target packages that the build needs but the
  runtime closure does not.
- `cross.host` names the build-root architecture. It must be the machine rpmx
  runs on; the default is the current host.
- `cross.triple` overrides the GNU triple (default: the target's).
- `cross.bootstrap: true` allows the build root and the sysroot to share an
  architecture (see [Native bootstrap](#native-bootstrap)).
- `cross.cflags`, `cross.cxxflags` and `cross.ldflags` add toolchain quirks
  that belong only to the cross build. Distribution-wide compiler policy comes
  from a package in the build root, not from the profile.

Step dependencies choose their root automatically. `needs.tools` and
`build.dependencies.tools` are build-root packages; `needs.host`,
`build.dependencies.host` and `build.dependencies.target` are sysroot packages.

With a single target in the profile, `rpmx build recipe.yaml` builds that
target without an `--arch` flag.

## What a step sees

The sandbox exports the toolchain and the sysroot path; a recipe passes flags,
it does not discover the toolchain:

| Variable | Use |
|---|---|
| `RPMX_CROSS=1` | Cross-compiling. `RPMX_EMULATED` is `0`. |
| `RPMX_TARGET_TRIPLE`, `RPMX_HOST_TRIPLE` | `--host`/`--build` values. |
| `RPMX_SYSROOT` | `/sysroot`, mounted read-only. |
| `CC`, `CXX`, `AR`, `LD`, `STRIP`, `OBJCOPY` | The cross toolchain. |
| `PKG_CONFIG_SYSROOT_DIR`, `PKG_CONFIG_LIBDIR` | pkg-config aimed at the sysroot. |
| `RPMX_CMAKE_TOOLCHAIN_FILE`, `RPMX_MESON_CROSS_FILE` | Generated toolchain files. |

Go needs no special step: `GOOS`/`GOARCH`/`CGO_ENABLED` and the `CGO_*` flags
make both a pure-Go and a cgo build produce a target binary. `go/build` accepts
`cgo: false` for a package that must be static; the default stays on. A cgo build links
the sysroot's libraries and is checked by the link-closure rule like any other
ELF object.

The built-in configure steps already use them: autotools adds
`--host=$RPMX_TARGET_TRIPLE`, CMake adds the toolchain file, Meson adds the
cross file, and `strip` uses `$STRIP`/`$OBJCOPY`. A custom step only needs the
flag its build system asks for:

```yaml
pipeline:
  - runs: |
      ./configure --host="$RPMX_TARGET_TRIPLE" --prefix="$PREFIX"
```

## What is checked

The link check runs for every assembled root, not just a cross sysroot, so a
native build that links something absent from its root fails the same way.

Two checks make a broken cross build fail instead of shipping:

- **ELF machine.** Every staged ELF object must match the target architecture.
  Without this, a build that silently used the host compiler produces an
  installable package full of host binaries.
- **Link closure.** Every `DT_NEEDED` of every staged ELF object must exist in
  the sysroot. This is the closest thing to `ldd` for a target that cannot run,
  and it catches both a missing dependency and a host library linked by
  mistake.

## Native bootstrap

A normal native profile has one root: rpmx runs `dnf install` in it and builds
against it. A bootstrap needs the opposite: a small seed build root with the
compiler, and a separate target sysroot with the replacement libc and headers,
even when both are the same architecture. Declare that explicitly:

```yaml
archs: [x86_64]
packages:
  install: [glibc, glibc-devel, zlib-devel]   # target sysroot
cross:
  bootstrap: true
  packages: [bash, coreutils, make, gcc, binutils]  # seed build root
```

`cross.bootstrap: true` permits `cross.host == archs`; without it the same
profile is rejected with `ROOT_006`, just like an accidental same-architecture
cross profile. The two roots still have different package lists and different
cache identities.

When the profile has no explicit `cross.triple`, a bootstrap uses the seed's
native command names (`gcc`, `g++`, `ar`, `ld`, `strip`, `objcopy`) with the
target triple kept for `--host` and for the SRPM's recorded provenance. Setting
`cross.triple` selects `<triple>-*` wrappers instead; a bootstrap ladder uses
that form once its newly built cross toolchain exists. The SRPM lock records
which form was used, so `rpmx rebuild` recreates the same environment.

## One profile, one build strategy

A `cross` block describes how one profile builds its target. A profile whose
`archs` include the host cannot also build the host: deriving a build root for
it fails with a message naming the profile. Build the host with a native
profile and each foreign target with a cross profile; `--arch all` on a cross
profile therefore selects that profile's targets only. The one exception is
`cross.bootstrap: true`, which deliberately keeps two same-architecture roots
separate.

The sysroot is a link-time artefact. Its payloads are unpacked without running
scriptlets, so it has no `ld.so.cache` or alternatives database; that is fine
for a compiler and wrong for anything that has to boot. Installed tests use a
real, scriptlet-installed test root instead.

## Locking both closures

`rpmx lock` records two roots for a cross recipe: the host build root and the
target sysroot, each with its requirement, closure digest and root digest.
`rpmx build --locked` fails with `changed: sysroot` if the target half drifts,
not just the build root.

## Emulation is announced

A profile without a `cross` block still builds a foreign target by executing
target binaries through qemu-user. That is a valid choice, but it now prints a
warning naming the alternative, because the difference is roughly 15x. A cross
profile never emulates, and `--unsafe-host-root` is refused for a cross build
because there is no assembled root to mount a sysroot into.

A cross profile is portable data: it can be loaded on another machine (for
example the target, to run tests) and only fails when a *build root* is derived
for a host that is not the current one.

## Rebuilding

A cross SRPM carries `sysroot.lock.json` and a `cross` block naming it and the
triple. `rpmx rebuild package.src.rpm` therefore needs
no profile: it assembles the target sysroot from the embedded lock (payloads
only, no emulation), runs the same native toolchain, and packages for the
sysroot's architecture. Without `--arch`, the output goes to
`packages/<srpm arch>/`.

## Tests

- `tests: stage: build` runs in the build sandbox and crosses like any step.
- `tests: stage: installed` runs the packaged target binary in a target test
  root assembled from the sysroot lock, so it needs the target architecture to
  be executable: binfmt on the build host, or a machine of that architecture.

### Building and testing separately

`rpmx build` runs the recipe's tests by default. A cross build executes
build-stage tests natively (they are host binaries) but cannot execute
installed-stage tests without binfmt, so `--skip-installed` keeps the native
tests while leaving the installed stage to the target. A fast host can also
package without testing at all:

```console
# build host: no installed tests, no binfmt
rpmx build package.yaml --skip-installed
# or: package only, no tests at all
rpmx build package.yaml --no-tests

# test host of the target architecture: no binfmt, no build root
rpmx test package.yaml --install-dir packages/aarch64 --profile native-aarch64.profile.yaml
```

The test host uses a *native* profile for its own architecture; the `cross`
block describes the build host and is not used there. To avoid resolving the
test root again, ship the `*.test-root.lock.json` the build writes and pin it:

```console
rpmx test package.yaml --install-dir DIR --test-root-lock DIR/pkg.test-root.lock.json --offline
```

The pinned form still needs the test root's packages in the local cache and the
builder image available, because it assembles the root from the lock. To move
the root itself instead, save it and ship the directory:

```console
rpmx build package.yaml --run-tests --save-test-root ./test-root
# on the machine that will run the tests:
rpmx test package.yaml --test-root ./test-root --test installed
```

`--test-root` needs no build root, profile, cache or network: the directory
contains the package under test and its lock sits beside it.

## Limitations

- A cross build needs a profile. An inline `build.root` describes one closure
  and cannot name a second, host-architecture package set.
- Recipe-specific target code generation, such as a configure script that runs
  what it compiles, must either provide a native answer or be run under binfmt.
