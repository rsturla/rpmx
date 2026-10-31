# rpmx

`rpmx` builds ordinary RPM packages from a strict YAML recipe. It produces a
native source RPM, runs builds in a locked root, and never invokes `rpmbuild`
or a spec file.

It is aimed at a small, image-first distribution: packages are built once,
signed, recorded in an SBOM, and composed into OCI images. It is an early
prototype, not production software.

## What it does

- **Strict recipes.** One schema-versioned `package.yaml`. Unknown fields are
  errors. No macros, no `%if`, no spec language.
- **Native SRPMs.** Every build writes a real `.src.rpm` containing the recipe,
  locks, sources, patches and step definitions. `rpmx rebuild` starts from that
  SRPM, not from loose sources.
- **Locked roots.** Package closure and the assembled root are pinned by
  NEVRA and SHA-512 digest. Builds run offline from the content-addressed
  cache.
- **Real tests.** Build-stage tests run in the build sandbox. Installed-stage
  tests run in a fresh test root with the produced RPMs installed and real
  dependency checking.
- **SBOM and CPE.** Every package embeds an SPDX 2.3 document at
  `/usr/share/sbom/<name>-<version>-<release>.spdx.json`. Build-level SPDX and
  CPE references are written beside the RPMs.
- **Optional signing.** Caller-supplied OpenPGP keys, applied after the build.
  `rpmx` never stores or discovers keys.
- **Repositories.** `rpmx repo` writes `repodata` and can sign `repomd.xml.asc`.
- **Explicit file metadata.** Empty directories, per-path owner/group/mode and
  file capabilities are recipe data.
- **Git sources.** Sources can be pinned to a full git commit and are archived
  deterministically before entering the cache.

## Status

Early prototype. The build, test, signing and repository paths are exercised
end to end on x86_64. Multi-arch runners and cross-compilation are not
implemented yet. The supported RPM feature set is intentionally small; see
[Known limitations](#known-limitations).

## Requirements

- Linux
- Rust 1.85 or newer
- `bubblewrap` for host sandboxing, or `podman` for root resolution and
  container-backed builds
- `dnf5` in the builder image used for root resolution
- `glibc` targets for now

You do **not** need `rpmbuild`, `rpm-sign`, or a spec file.

## Build from source

```console
cargo build --release
./target/release/rpmx --help
```

The binary is `target/release/rpmx`.

## Quick start

Resolve a build root and assemble it offline:

```console
./target/release/rpmx root resolve examples/fedora-44-build.profile.yaml \
  --output root.lock.yaml

./target/release/rpmx root assemble root.lock.yaml \
  --root ./root --output root.assembled.yaml
```

Build a package end to end, including its tests:

```console
./target/release/rpmx build examples/pyhello/package.yaml \
  --root-lock root.assembled.yaml --root ./root \
  --output ./out --run-tests
```

Output:

```text
out/pyhello-1.0.0-1.noarch.rpm
out/src/pyhello-1.0.0-1.src.rpm
out/pyhello-1.0.0-1.build.spdx.json
out/pyhello-1.0.0-1.test-root.lock.json
```

Rebuild from the SRPM without the recipe:

```console
./target/release/rpmx rebuild out/src/pyhello-1.0.0-1.src.rpm \
  --root ./root --output ./rebuilt
```

## A recipe

```yaml
schema: 1
package:
  name: hello
  version: "1.0"
  release: 1
  summary: A tiny hello package
  license: MIT
  source-date-epoch: 1704067200
  cpe:
    vendor: example
    product: hello

build:
  dependencies:
    tools: [gcc, make, sh]
    tests: [gcc]
  pipeline:
    - runs: |
        cd hello-1.0
        make
        make install DESTDIR=${stage}
    - uses: strip

outputs:
  - name: hello
    files:
      include:
        - /usr/bin/hello
        - /usr/share/man/**
    directories:
      - path: /var/lib/hello
        mode: "0750"
        user: root
        group: root
    attributes:
      - path: /usr/bin/hello
        mode: "0755"
        user: root
        group: root

tests:
  - name: installed
    stage: installed
    argv: ["hello", "--version"]
```

See [`examples/`](examples/) for tested recipes, including a shared library,
Autotools packages, Lua, jq, a noarch Python wheel, and a systemd service.

## Commands

| Command | Purpose |
|---|---|
| `rpmx lint FILE` | Validate a recipe and expand its matrix. |
| `rpmx plan FILE` | Show cells, outputs and expanded steps. |
| `rpmx fetch FILE` | Populate the content-addressed cache. |
| `rpmx source FILE --root-lock L --root R` | Assemble the deterministic source snapshot. |
| `rpmx srpm FILE --sources DIR` | Write a native SRPM. |
| `rpmx build FILE --root-lock L --root R` | Run the full source → SRPM → binary pipeline. |
| `rpmx build FILE --stage DIR` | Package an already-staged tree. |
| `rpmx rebuild SRPM --root R` | Rebuild binaries from an SRPM. |
| `rpmx root resolve PROFILE` | Pin a package closure. |
| `rpmx root assemble LOCK` | Build the root offline from the cache. |
| `rpmx root digest ROOT` | Compute the deterministic root digest. |
| `rpmx keygen` | Generate an OpenPGP signing key. |
| `rpmx sign --signing-key KEY RPM…` | Sign existing RPMs. |
| `rpmx repo --rpms DIR --output DIR` | Write and optionally sign repository metadata. |
| `rpmx cache gc --lock L --recipe R` | Remove unreferenced cache blobs. |

## Policies and artifacts

The profile carries artifact policy:

```yaml
debuginfo: false
sbom: true
```

- `debuginfo: true` injects `-g`, extracts debug sections and emits a
  `<name>-debuginfo` package.
- `sbom: true` embeds SPDX in each RPM and writes a build-level SPDX document.
- Signing is explicit:

```console
./target/release/rpmx keygen --output key.asc
./target/release/rpmx build package.yaml --root-lock root.assembled.yaml \
  --root ./root --signing-key key.asc --output ./out
./target/release/rpmx repo --rpms ./out --output ./repo --signing-key key.asc
```

`rpmx` does not sign automatically and does not hold keys.

## Sources and patches

Sources are content-addressed and support `https`, `file`, `go-module` and
`git` locators. Git sources require a full commit:

```yaml
sources:
  - name: mytool
    kind: git
    digest: "sha512:..."
    locators:
      - kind: git
        url: https://github.com/example/mytool.git
        commit: 0123456789abcdef0123456789abcdef01234567
```

Patches are a top-level list and are applied before source assembly.

## Runners and architectures

Every command distinguishes the **host architecture** (where `rpmx` runs) from
the **target architecture** (the packages it produces). `--arch` defaults to
the native architecture and drives templates, SRPM metadata, outputs and the
repository.

The runner is an explicit interface. The native bubblewrap and container
runners currently accept only `host == target`; a different target fails with
`RPMX-RUNNER-002` rather than silently executing the wrong architecture. The
same interface is where an emulated target runner will plug in, without
changing recipes or the sandbox callers:

```rust
pub trait Runner {
    fn name(&self) -> &'static str;
    fn supports(&self, host: Arch, target: Arch) -> bool;
    fn run(&self, request: &RunnerRequest<'_>) -> Result<()>;
}
```

The sandbox passes `RPMX_HOST_ARCH`, `RPMX_TARGET_ARCH` and `RPMX_CROSS` to
steps. Root assembly and installed tests refuse a foreign target until an
emulated runner exists.

## Sandbox

Build steps run with no network, a cleared environment, a read-only root, and
writable `/tmp/build` and `/tmp/stage`. Steps have a wall-clock timeout.
Source assembly uses a separate locked root. `--unsafe-host-root` is the
explicit escape hatch.

## Known limitations

- Weak and rich dependencies are not implemented.
- SELinux labels, `%ghost`, `%lang`, multilib and arbitrary xattrs are not
  implemented. File capabilities are.
- `-debugsource`, build-id debug links and per-subpackage debuginfo are not
  implemented.
- Cross-compilation and multi-arch runners are not implemented yet.
- Repository snapshots, module streams and seccomp/cgroup enforcement are not
  implemented.
- CPE is emitted in SBOMs, not in the RPM header.
- The language pipelines are partial: Cargo, npm, Go and Python have
  build/vendor steps, but not every ecosystem has a complete source path.

## Documentation

- [`docs/00-design-study.md`](docs/00-design-study.md) — the design study and
  rationale.
- [`docs/03-design.md`](docs/03-design.md) — the detailed model, CLI and
  semantics.
- [`docs/06-build-inputs.md`](docs/06-build-inputs.md) — sources, CAS and
  source assembly.
- [`docs/07-srpm-first.md`](docs/07-srpm-first.md) — the SRPM-first build
  model.
- [`docs/09-sandboxing.md`](docs/09-sandboxing.md) — the sandbox contract.
- [`docs/14-build-root.md`](docs/14-build-root.md) — root resolution and
  assembly.
- [`examples/README.md`](examples/README.md) — the tested example corpus.

## License

Dual-licensed under Apache-2.0 or MIT. See [`LICENSE-APACHE`](LICENSE-APACHE)
and [`LICENSE-MIT`](LICENSE-MIT).
