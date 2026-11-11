# rpmx

[![CI](https://github.com/rsturla/rpmx/actions/workflows/ci.yml/badge.svg)](https://github.com/rsturla/rpmx/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

Build RPM packages from a strict YAML recipe. No spec file, no `rpmbuild`.

`rpmx` is the package builder behind
[Eternal Linux](https://github.com/rsturla/eternal-os). A package is one
schema-versioned `package.yaml`; every build produces a native source RPM, runs
in a locked and offline build root, and embeds an SPDX SBOM. It is built for
small, image-first distributions: packages are built once, signed, and composed
into OCI images.

> [!WARNING]
> Early prototype, not production software. See
> [current limitations](docs/usage/reference.md#current-limitations).

## Features

- **Strict recipes.** One `package.yaml`, optionally split into one file per
  section with `include:`. Unknown fields are errors. No macros, no `%if`, no
  spec language.
- **SRPM-first.** Every build writes a real `.src.rpm` containing the recipe,
  locks, sources, patches and steps; `rpmx rebuild` starts from it.
- **Locked, offline builds.** The root closure is pinned by NEVRA and SHA-512
  digest and builds run in a sandbox with no network.
- **Explicit test phases.** Source cases run during builds. Standalone acceptance
  checks existing RPMs in isolated roots with real dependency checking.
- **Generated metadata.** Dependencies from ELF sonames, pkgconfig, shebangs,
  Python, Perl and desktop/AppStream data; SPDX SBOMs; optional OpenPGP signing
  and repository metadata.

## Install

```console
cargo install --git https://github.com/rsturla/rpmx --locked rpmx
```

Linux, Rust 1.88 or newer, `bubblewrap`, `podman` and `dnf5` on the host.
`rpmx doctor` checks the rest. You do not need `rpmbuild`, `rpm-sign`, or a
spec file.

## Quick start

```console
rpmx init hello
cd hello

rpmx lint     # every rule, one pass
rpmx build    # resolve root, source, SRPM, build, test, package, index
rpmx test --install-dir packages/x86_64  # accept existing RPMs
```

The first build resolves the root over the network; later builds reuse the
cache. Artifacts land in `packages/<arch>/` with repository metadata already
written. Pin what was resolved and make CI reject drift:

```console
rpmx lock
rpmx build --locked
```

## A recipe

GNU Hello, end to end: fetch and verify a source archive, configure and build
it with the Autotools and Make steps, copy the license, strip, and test the
installed package.

```yaml
schema: 2

package:
  name: hello
  version: "2.12.1"
  release: 1
  summary: The GNU hello program
  license: GPL-3.0-or-later
  upstream: https://www.gnu.org/software/hello/
  cpe:
    vendor: gnu
    product: hello

sources:
  - name: hello-2.12.1.tar.gz
    kind: archive
    digest: "sha512:f871e5f8f64b0633ee45855c886ddf37565ae7a13d68a2d7d2df007e508355fcc85736b93c0274c4fed5e628a94d8a3f699925c7f44857dcd0aba78b747749e2"
    locators:
      - kind: https
        url: https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz
      - kind: https
        url: https://mirrors.kernel.org/gnu/hello/hello-2.12.1.tar.gz
    extract:
      format: tar.gz
      strip-components: 1

build:
  root:
    profile: profiles/fedora-44.yaml
  packages: [gcc, make, bash, coreutils, gawk]
  pipeline:
    - uses: autotools/configure
    - uses: make/build
    - uses: make/install
    - uses: license/copy
      with:
        files: [COPYING]
        outputs: [hello]
    - uses: strip

outputs:
  - name: hello
    files:
      license:
        - /usr/share/licenses/hello/COPYING
      attributes:
        - path: /usr/bin/hello
          mode: "0755"
          user: root
          group: root

tests:
  - name: installed
    argv: ["hello", "--version"]
```

Every field is documented in [`docs/usage/recipes.md`](docs/usage/recipes.md).
[`examples/`](examples/) contains runnable recipes for libraries, Go, Python,
systemd and more, plus clearly marked historical design sketches.

## Documentation

| Guide | Contents |
|---|---|
| [Getting started](docs/usage/getting-started.md) | Requirements, first package, command map |
| [Recipes](docs/usage/recipes.md) | The complete `package.yaml` reference |
| [Profiles and roots](docs/usage/profiles-and-roots.md) | Repositories, authentication, resolution |
| [Building](docs/usage/building.md) | Artifacts, locks, signing, repositories, SRPMs |
| [Testing](docs/usage/testing.md) | Source and package cases, fixtures, replay, reports |
| [Cross-compilation](docs/usage/cross-compilation.md) | Native toolchain, target sysroot |
| [Bootstrap](docs/usage/bootstrap.md) | Seed-based toolchain ladders |
| [Debugging](docs/usage/debugging.md) · [Troubleshooting](docs/usage/troubleshooting.md) · [Reference](docs/usage/reference.md) | `plan`, `explain`, `doctor`, errors, limits |

Design notes and contribution rules live in
[`docs/contributing/`](docs/contributing/) and [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Security

`rpmx` is pre-release software with no production support window. Report
sandbox, parsing, signature, digest or key-handling issues privately; see
[`SECURITY.md`](SECURITY.md).

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT).
Contributions are accepted under the same terms. The vendored `rpm-rs` crate
and the Rust dependency license summary are in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
