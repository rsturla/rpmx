# Getting started

## What rpmx does

`rpmx` turns a strict YAML recipe into ordinary RPMs without `rpmbuild` or a
spec file:

```
package.yaml + profile + sources
        |
        v
  source snapshot  ->  native SRPM  ->  binary RPMs + SBOM + repodata
```

Every build passes through a source RPM. The SRPM contains the recipe, the
package lock, the resolved step definitions, the sources and patches. `rpmx
rebuild` starts from that SRPM, so it does not need the recipe checkout.

The three things to remember:

1. The **recipe** is intent. It names sources, steps, outputs and tests.
2. The **profile** (or an inline `build.root`) describes the machine that runs
   the build.
3. The **lock** records what those two resolved to. `--locked` refuses to drift
   from it.

---

## Requirements

- Linux, x86_64 or aarch64 host.
- Rust 1.88+ to build rpmx (the vendored rpm crate requires it).
- `bubblewrap` for native steps, or `podman` for root resolution and
  image-backed rebuilds.
- `dnf5` in the builder image used for root resolution.
- `tar`, `findutils`, `binutils` and the usual build tools inside the root.
- For foreign-architecture builds: `qemu-system-<arch>`, `qemu-user-static` with
  binfmt, `ssh`, `ssh-keygen`, `mke2fs`, `debugfs` and `tar`. The QEMU appliance
  is resolved from the profile repositories; do not add kernel or SSH packages
  to the profile.

Check the host:

```console
rpmx doctor
rpmx doctor --json
rpmx doctor --strict     # non-zero when a required feature is missing
```

Build:

```console
cargo build --release
./target/release/rpmx --help
```

---

## First package

```console
rpmx init hello
cd hello

rpmx lint
rpmx build
rpmx test
```

`rpmx init` writes:

```
hello/
  package.yaml
  profiles/fedora-44.yaml
  pipelines/          (empty; local steps go here)
  README.md
  .gitignore
```

`rpmx init --template python` and `--template metapackage` are also available.
`--profile NAME` changes the generated profile name and reference.
`--force` overwrites existing files; without it, `init` refuses.

Output:

```
packages/x86_64/hello-0.1.0-1.fc44.x86_64.rpm
packages/x86_64/src/hello-0.1.0-1.fc44.src.rpm
packages/x86_64/hello-0.1.0-1.fc44.build.spdx.json
packages/x86_64/repodata/repomd.xml
```

The first build resolves and assembles the root (network, a few minutes). The
next build reuses the cached root and is fast. The four lines above are stdout;
leveled progress lines such as `INFO root`, `INFO wrote` and `INFO done`, and
guest output, go to stderr.

---

## Command map

| Command | Use it for |
|---|---|
| `rpmx init [DIR]` | Start a package. |
| `rpmx lint [FILE]` | Check every rule, expand the matrix. |
| `rpmx plan [FILE]` | Show cells, outputs, tests and expanded commands. |
| `rpmx build [FILE]` | Resolve root, fetch, snapshot, SRPM, build, package, index. |
| `rpmx test [FILE]` | Run tests, building first when needed. |
| `rpmx rebuild SRPM` | Rebuild a source RPM, assembling its embedded root. |
| `rpmx lock [FILE]` | Write or check `package.lock.json`. |
| `rpmx explain TOPIC` | Explain policy, root, steps, matrix, requires or files. |
| `rpmx bump VERSION [FILE]` | Update version, source URLs and digests. |
| `rpmx repo` | Write repodata for a directory of RPMs. |
| `rpmx shell` | Open a shell with the build mounts. |
| `rpmx steps list\|show` | Inspect the built-in step library. |
| `rpmx doctor` | Check host tools, runners, cache and cached QEMU appliances. |
| `rpmx fetch` | Populate the source cache. |
| `rpmx source` | Produce the deterministic source snapshot. |
| `rpmx srpm` | Write an SRPM from an existing snapshot. |
| `rpmx root resolve\|assemble\|digest` | Explicit root plumbing for debugging. |
| `rpmx keygen` / `rpmx sign` | Keys and package signatures. |
| `rpmx cache gc` | Remove unreferenced cache blobs. |

Every recipe command defaults `FILE` to `./package.yaml`. `build`, `test` and
`rebuild` default output to `./packages/<arch>/` and jobs to all available
CPUs.

Global options:

| Option | Meaning |
|---|---|
| `--pipeline-dir DIR` | Where local step files live (default: `<recipe>/pipelines`). |
| `--keep-workdir` | Keep temporary directories and print their paths. |
| `--debug` | Echo step commands before running them. |
| `--verbose` | List resolved detail such as every build-root package. |
| `--color MODE` | Color progress: `auto`, `always` or `never`. |

---
