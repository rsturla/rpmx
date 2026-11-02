# rpmx

`rpmx` builds ordinary RPM packages from a strict YAML recipe. It produces a
native source RPM, runs builds in a locked root, and never invokes `rpmbuild`
or a spec file.

It is aimed at a small, image-first distribution: packages are built once,
signed, recorded in an SBOM, and composed into OCI images. It is an early
prototype, not production software.

## What it does

- **Strict recipes.** One schema-versioned `package.yaml`, optionally split
  into one file per section with `include:`. Unknown fields are errors. No
  macros, no `%if`, no spec language.
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
- **Automatic dependencies.** ELF sonames and symbol versions, pkgconfig,
  interpreter shebangs, Python ABI/distribution names, Perl modules,
  desktop/AppStream metadata and debuginfo build-ids are derived from the
  packaged files. Recipe-declared dependencies are merged, and a per-output
  `generators:` block can disable a class or exclude one generated entry.
- **Git sources.** Sources can be pinned to a full git commit and are archived
  deterministically before entering the cache.

## Status

Early prototype. The build, test, signing and repository paths are exercised
end to end on x86_64, and aarch64 builds run through qemu-user binfmt. Cross
compilation is not implemented. The supported RPM feature set is intentionally
small; see [Known limitations](#known-limitations).

## Requirements

- Linux
- Rust 1.88 or newer
- `bubblewrap` for host sandboxing, or `podman` for root resolution and
  container-backed builds
- `dnf5` in the builder image used for root resolution
- `glibc` targets for now
- `qemu-system-<arch>` on PATH (or `RPMX_QEMU_SYSTEM`) for foreign-architecture builds
- `qemu-user-static` with binfmt for root assembly and test-root installation
- `ssh`, `ssh-keygen`, `mke2fs`, `debugfs` and `tar` for the QEMU workspace transport

You do **not** need `rpmbuild`, `rpm-sign`, or a spec file.

## Build from source

```console
cargo build --release
./target/release/rpmx --help
```

The binary is `target/release/rpmx`.

## Quick start

```console
cargo build --release
./target/release/rpmx init hello
cd hello

./target/release/rpmx lint         # every rule, one pass
./target/release/rpmx build        # resolve root, build, package, index
./target/release/rpmx test         # build- and installed-stage tests
```

`rpmx init` writes a recipe, a root profile, a README and a `.gitignore`. The
recipe names its root (`build.root.profile`); rpmx resolves and assembles it on
the first build, then reuses it from the cache.

Output lands in `packages/<arch>/`:

```text
packages/x86_64/hello-0.1.0-1.fc44.x86_64.rpm
packages/x86_64/src/hello-0.1.0-1.fc44.src.rpm
packages/x86_64/hello-0.1.0-1.fc44.build.spdx.json
packages/x86_64/repodata/repomd.xml
```

Pin what was resolved and make CI reject drift:

```console
./target/release/rpmx lock
./target/release/rpmx build --locked
```

Rebuild from the SRPM without the recipe. The root lock is embedded, so the
root is assembled from the cache automatically:

```console
./target/release/rpmx rebuild packages/x86_64/src/hello-0.1.0-1.fc44.src.rpm
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
| `rpmx init [DIR]` | Create a package that builds on the first try. |
| `rpmx bump VERSION` | Update the version, source URLs and digests. |
| `rpmx lint [FILE]` | Validate a recipe, report every rule, and expand its matrix. |
| `rpmx lock [FILE]` | Write `package.lock.json`; `--check` verifies it. |
| `rpmx plan [FILE]` | Show cells, outputs and expanded steps. |
| `rpmx build [FILE]` | Resolve the root, build, package and index. |
| `rpmx test [FILE]` | Run tests; `--test` selects and `--install-dir DIR` tests existing RPMs. |
| `rpmx rebuild SRPM` | Rebuild binaries from an SRPM, assembling its root. |
| `rpmx explain TOPIC` | Explain policy, root, steps, matrix, requires or files. |
| `rpmx shell` | Enter the recipe's build sandbox with the build mount layout. |
| `rpmx steps list\|show` | Browse the built-in step library. |
| `rpmx doctor` | Check host tools, runners, cache and cached QEMU appliances. |
| `rpmx fetch [FILE]` | Populate the content-addressed cache. |
| `rpmx source [FILE]` | Assemble the deterministic source snapshot. |
| `rpmx srpm [FILE]` | Write a native SRPM, assembling sources when `--sources` is omitted. |
| `rpmx root resolve\|assemble\|digest` | Explicit root plumbing for debugging. |
| `rpmx keygen` / `rpmx sign` | Generate a key; sign existing RPMs. |
| `rpmx repo --rpms DIR --output DIR` | Write and optionally sign repository metadata. |
| `rpmx cache gc --lock L --recipe R` | Remove unreferenced cache blobs. |

Recipe commands default `FILE` to `./package.yaml`. `build`, `test` and
`rebuild` default their output to `./packages/<arch>/`, default `--jobs` to the
host CPU count (exported to build steps as `JOBS`), and write repository
metadata unless `--no-index` is set. Progress is leveled `INFO`/`WARN` output on
stderr, including guest output; stdout carries only artifact paths or the
`--json` document. `--keep-workdir` preserves a failed workspace and prints a
labelled path per directory; `--debug` echoes the commands a step runs;
`--verbose` lists resolved detail and step durations; `--color=auto|always|never`
controls ANSI color.

## Local repositories

A profile repository may point at a directory on the host:

```yaml
repositories:
  - id: local
    baseurl: file:///srv/rpmx/packages
```

rpmx mounts that directory read-only in the resolver container, so a package
built earlier can satisfy another package's build dependency:

```console
rpmx repo --rpms out/ --output /srv/rpmx/packages
rpmx root resolve local.profile.yaml
```

The same applies to a local `gpgkey`; it is copied into the resolver work
directory and referenced by `file://`.

## Authenticated repositories

A profile can authenticate to an RPM repository with a TLS client certificate
(RHEL CDN), basic auth (Satellite or a private mirror), or a proxy. Secret
values are never written into the recipe, lock or SRPM. The profile names an
`env:NAME` reference or an absolute file path; rpmx resolves it at resolve time.

```yaml
repositories:
  - id: rhel-baseos
    baseurl: https://cdn.redhat.com/content/dist/rhel9/$releasever/$basearch/baseos/os
    gpgkey: file:///etc/pki/rpm-gpg/RPM-GPG-KEY-redhat-release
    repo-gpgcheck: true
    auth:
      ca: /etc/pki/entitlement/...-ca.pem
      cert: env:RHEL_CLIENT_CERT
      key: env:RHEL_CLIENT_KEY
```

Basic auth uses `auth.username` and `auth.password`. The password must be an
`env:NAME` reference or a file path, so a literal secret is a profile error.
Proxy credentials use `auth.proxy`, `auth.proxy-username` and
`auth.proxy-password`.

Credential files are copied into a private temporary directory, mounted
read-only into the resolver container, and removed when resolution finishes.
They are never mounted into the build sandbox, which has no network. Repository
URLs must not embed a user name or password; use the `auth` block instead.

Credentials are honoured only in a profile the user controls: one passed with
`--profile` or found under `$XDG_CONFIG_HOME/rpmx/profiles`. A profile selected
by a recipe cannot carry `auth`, so an untrusted recipe cannot redirect the
user's credentials to its own repository.

`root assemble` and `rebuild` use payloads from the content-addressed cache, so
they do not need the credentials. A host without entitlement can build from a
cache populated by an entitled host.

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

The runner is session-based. A runner creates an execution environment once;
the session runs every step and is destroyed at the end:

```rust
pub trait Runner {
    fn name(&self) -> &'static str;
    fn supports(&self, host: Arch, target: Arch) -> bool;
    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>>;
}

pub trait Session {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()>;
    /// Copy host-visible outputs back from the session. Native bind-mounted
    /// sessions do nothing; the VM tars the workspace and stage back.
    fn sync_out(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}
```

Three backends are implemented:

- **bubblewrap** — native `host == target`, no daemon, namespaces, no network.
- **podman** — native `host == target`, a long-lived container with
  `--network=none`, a read-only root, dropped capabilities and no new
  privileges.
- **qemu-system** — a full VM for foreign targets. It boots a separate
  runner-owned appliance (kernel, initramfs, systemd, SSH and tar), attaches a
  disposable copy of the build root as a second disk, and runs steps on a
  chrooted build channel. A separate control channel mounts the disk and moves
  the workspace and stage. There is no 9p or virtiofs share. The guest has one
  host-only NIC (`restrict=on`) whose only purpose is the two SSH port
  forwards; it cannot reach the host network or the internet.

A foreign target uses qemu-system automatically. The appliance is resolved from
the same profile repositories, releasever and builder image, with a package
list owned by rpmx. The profile does not list kernel or SSH packages. Explicit
`rpmx root resolve` includes the appliance by default; automatic native builds
skip it. Runner-owned assets are a generic lock/cache concept; the QEMU
appliance is the first kind. `rpmx runner resolve|assemble|inspect` handles
assets independently (the old appliance subcommand remains an alias), and
`rpmx root assemble` caches the appliance with the build root. Pass
`--runner-asset-root DIR` to `build`, `source`, `test` or `rebuild` to use a
pre-assembled asset. Local x86_64 hosts can build aarch64
with `qemu-system-aarch64` and TCG; native aarch64 CI uses KVM when available.
`RPMX_QEMU_SYSTEM` overrides the qemu-system binary path.

Root assembly and test-root installation still use podman's `--arch` and the
host binfmt handler; build steps do not.

The sandbox passes `RPMX_HOST_ARCH`, `RPMX_TARGET_ARCH` and `RPMX_CROSS` to
steps. A runner that cannot execute the target fails with `RPMX-RUNNER-002`
before any step runs.

## Sandbox

Build steps run with no network, a cleared environment, a read-only root, and
writable `/tmp/build` and `/tmp/stage`. Steps have a wall-clock timeout.
Source assembly uses a separate locked root. `--unsafe-host-root` is the
explicit escape hatch.

## Known limitations

- Rich dependencies are not implemented; weak dependencies
  (`recommends`, `suggests`, `supplements`, `enhances`) are.
- Dependency generators cover ELF, pkgconfig, shebangs, Python, Perl,
  desktop/AppStream and debuginfo. Fontconfig, OCaml, sysusers/usergroup and
  rpm macro provides are not generated; Python emits versioned
  `pythonX.Ydist(...)` names only; a distro that wants a major-only alias or
  extras subpackages declares them on its runtime package. A
  `#!/usr/bin/env name` shebang requires `/usr/bin/env`, not `name`, and
  scriptlet dependencies are not generated.
- SELinux labels, `%ghost`, `%lang`, multilib and arbitrary xattrs are not
  implemented. File capabilities are.
- `-debugsource`, build-id debug links and per-subpackage debuginfo are not
  implemented.
- Cross-compilation is not implemented. Native builds use bubblewrap; foreign
  builds use a full qemu-system VM and require `qemu-system-<arch>` and a
  locked, assembled transport appliance. The build root itself is not
  bootable and does not need kernel or SSH packages.
- Repository snapshots, module streams and seccomp/cgroup enforcement are not
  implemented.
- CPE is emitted in SBOMs, not in the RPM header.
- The language pipelines are partial: Cargo, npm, Go and Python have
  build/vendor steps, but not every ecosystem has a complete source path.

## Documentation

Usage:

- [`docs/usage/`](docs/usage/) — start here: getting started, recipe reference,
  profiles and roots, building, testing, debugging and troubleshooting.
- [`examples/README.md`](examples/README.md) — the tested example corpus.

Contributing and design:

- [`CONTRIBUTING.md`](CONTRIBUTING.md) — build/test commands and design rules.
- [`RELEASING.md`](RELEASING.md) — the release and packaging checklist.
- [`docs/contributing/`](docs/contributing/) — the design study, format
  choices, build model, sandbox contract and the UX audit against Melange.

## Security

`rpmx` is pre-release software with no production support window. Report
sandbox, parsing, signature, digest or key-handling issues privately; see
[`SECURITY.md`](SECURITY.md).

## License

Dual-licensed under Apache-2.0 or MIT. See [`LICENSE-APACHE`](LICENSE-APACHE)
and [`LICENSE-MIT`](LICENSE-MIT). Contributions are accepted under the same
terms.

The vendored `rpm-rs` crate and the Rust dependency license summary are in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
