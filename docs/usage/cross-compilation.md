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
- `cross.cflags`, `cross.cxxflags`, `cross.ldflags` add the distribution's own
  toolchain flags.

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
make both a pure-Go and a cgo build produce a target binary. A cgo build links
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

Two checks make a broken cross build fail instead of shipping:

- **ELF machine.** Every staged ELF object must match the target architecture.
  Without this, a build that silently used the host compiler produces an
  installable package full of host binaries.
- **Link closure.** Every `DT_NEEDED` of every staged ELF object must exist in
  the sysroot. This is the closest thing to `ldd` for a target that cannot run,
  and it catches both a missing dependency and a host library linked by
  mistake.

## Rebuilding

A cross SRPM carries `sysroot.lock.json` and a `cross` block naming it, the
triple and the toolchain flags. `rpmx rebuild package.src.rpm` therefore needs
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

`rpmx build` runs no tests unless `--run-tests` is passed, so a fast host can
produce packages and a capable machine can test them:

```console
# build host: no installed tests, no binfmt
rpmx build package.yaml

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
builder image available, because it assembles the root from the lock.

## Limitations

- A cross build needs a profile. An inline `build.root` describes one closure
  and cannot name a second, host-architecture package set.
- Recipe-specific target code generation, such as a configure script that runs
  what it compiles, must either provide a native answer or be run under binfmt.
