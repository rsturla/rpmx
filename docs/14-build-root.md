# Build-root resolution and assembly

## 1. Decision

`rpmx` owns the root contract, but it does not reimplement RPM's solver.
Root assembly runs **dnf5/libdnf5 inside a rootless container**, not on the
host, not through mock/koji, and not by extracting RPMs with `rpm2cpio`.

A builder OCI image is a distribution and cache form of a root, not the source
of truth. The source of truth is the resolved closure in the lock.

## 2. Pipeline

```
profile
  repositories, keys, releasever, arch, packages
        |
        v
resolve            network allowed only through the proxy
                   dnf transaction in a rootless container
                   -> NEVRAs, RPM payload digests, repo metadata digests
        |
        v
snapshot           RPMs and metadata into the CAS
        |
        v
assemble           offline dnf --installroot from the local snapshot
                   rootless container, no network
        |
        v
verify             compare closure and root digest
        |
        v
seal               root read-only; mount into the build sandbox
```

Resolve and assemble are separate stages. A rebuild never needs a repository.

## 3. Why dnf

| Concern | dnf/libdnf5 | rpm directly | rpm2cpio extraction |
|---|---|---|---|
| Dependency solving | yes | no | no |
| Obsoletes, conflicts, file conflicts | yes | partial | no |
| Scriptlets (`ldconfig`, users, alternatives) | yes, in the root | yes, but ordering is manual | no |
| Repo keys and metadata | yes | manual | no |
| Offline from a pinned set | yes, with a local repo | yes, if ordered | yes |
| Host root required | no, inside a rootless container | no, inside a userns | no |

`dnf` is a front end to libsolv and librpm. The design should use it as a
backend, not treat it as the contract.

## 4. What "rpmx resolves" means

- The profile declares repositories, keys, releasever, arch and packages.
- Resolution is native-only: dnf is told to exclude the i686 compat arch on
  x86_64, and any package whose arch is neither the target nor `noarch` is an
  error (`RPMX-ROOT-009`). Without this, dnf pulls the i686 companions of
  multilib-capable packages into an otherwise x86_64 root.
- `rpmx` generates the dnf configuration, runs the transaction, and captures
  the result.
- The lock records repo metadata digests, exact NEVRAs, RPM payload digests and
  the closure digest.
- A builder image is accepted only when its recorded root digest matches the
  lock.

The solver remains an implementation detail, like `rpm` for the writer.

## 5. Root lock

```yaml
document: rpmx-root-lock
version: 1
profile: fedora-44
archs: [x86_64]
releasever: "44"
dist_tag: .fc44
options:
  weak_deps: false
  docs: false
repositories:
  - id: fedora
    metalink: https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever&arch=$basearch
packages:
  x86_64:
    - name: bash
      repo_id: fedora
      evr: 5.3.9-3.fc44
      checksum: sha512:1985877...
      size: 1985877
      srpm: bash-5.3.9-3.fc44.src.rpm
  noarch:
    - name: fedora-release
      repo_id: updates
      evr: 44-18
      checksum: sha512:...
      size: 12761
closure_digest: sha512:...
root_digest: sha512:...        # added by `root assemble`
builder:
  image: registry.fedoraproject.org/fedora:44
  image_digest: sha256:a4907bdd...
```

- Packages are grouped by package architecture. `noarch` packages get their own
  group.
- `evr` includes the epoch only when it is non-zero.
- `checksum` is the SHA-512 of the RPM file, the same digest the CAS uses.
- `location` is deliberately absent. It is resolved from the repository at
  download time; recording it would make the lock mirror-specific. A consumer
  resolves a payload by `repo_id` + NEVRA and verifies `checksum`.
- `options` records the resolve policy that affects the package set and the
  installed files. `root assemble` replays it. It is deliberately outside
  `closure_digest`: weak dependencies already change the package list, and docs
  change the assembled tree, which `root_digest` covers.
- `closure_digest` covers `releasever`, `archs`, each repository `id` and URL,
  and each package (`arch`, `name`, `evr`, `repo_id`, `checksum`, `size`,
  `srpm`, `module`). It excludes `builder`, `root_digest`, `options`,
  `dist_tag` and `location`, so two resolutions of the same inputs agree byte
  for byte. `dist_tag` is target provenance: it changes package naming, not the
  resolved root contents. `rpmx srpm --root-lock` uses the lock's tag when
  neither the profile nor `--dist-tag` supplied one; a conflicting flag is an
  error (`RPMX-SRPM-012`).
- `root_digest` verifies the assembled tree, with volatile state excluded or
  normalized. It is written by `root assemble`, not by `root resolve`.
- `root_digest` alone is not the identity; scriptlets and filesystem state make
  it weaker than the closure.

## 6. Assembly rules

- No dnf in the build sandbox. Root assembly is a separate phase and container.
- No host root. dnf runs with `--installroot` inside a rootless container.
- No network. Assembly uses only payloads from the CAS.
- No repository metadata at assembly time. The lock is the closure; dnf installs
  the cached RPM files directly and resolves only within that set.
- Weak dependencies and docs follow the lock's `options`.
- `releasever` and arch come from the lock.
- The builder image is accepted only when its digest matches the lock.
- The assembled package set is compared against the lock before the root is
  accepted.
- For bwrap and podman, the root is mounted read-only in the build sandbox.
  For QEMU, the root is a disposable writable disk copy: build steps can modify
  it, but those changes cannot escape the VM and do not persist.

## 7. Offline assembly

`rpmx root assemble` does not generate repository metadata. The lock already
names every package, so the cached RPM files are passed to dnf as local
packages:

```
lock -> CAS payloads -> dnf install --installroot=/root <files...>
```

with an empty `reposdir` and `--network=none`. Dnf reads the headers, orders the
transaction, runs scriptlets and has no repository to fall back to. This avoids
a `createrepo_c` dependency and removes the risk of resolving to a package the
lock did not record.

The container is read-only with a writable root mount, a `/tmp` tmpfs and only
the capabilities rpm scriptlets need: `CHOWN`, `DAC_OVERRIDE`, `FOWNER`,
`FSETID`, `SETGID`, `SETUID`, `SYS_CHROOT`, `SETFCAP`, `SYS_ADMIN` and
`MKNOD`. `SYS_ADMIN` and `MKNOD` are used by systemd and dracut scriptlets;
`tsflags=nocaps` keeps rpm from writing file capabilities into an unprivileged
install root. Dropping back to the smaller capability set makes the transaction
fail.

The local files are not OpenPGP-checked (dnf reports that), but every payload
was verified against its SHA-512 in the CAS before staging, and the installed
NEVRA set is compared with the lock afterwards.


## 8. Root digest

`root_digest` is a SHA-512 over a canonical listing of the assembled tree:

```
d|path|mode
f|path|mode|sha512(content)
l|path|target
```

Lines are sorted by path, so the digest does not depend on traversal order.
Mtimes are excluded because scriptlet-created directories do not carry stable
times. Ownership is excluded because a user namespace reports the same file with
a different uid/gid depending on how it was entered. Ownership and capabilities
are not covered; they are pinned by the package headers and are a candidate for
a future lock field. The following are excluded because package managers or boot write them,
not the package payload:

- `proc`, `sys`, `dev`, `run`, `tmp`
- `usr/lib/sysimage/rpm`, `usr/lib/sysimage/libdnf5`, `var/lib/rpm`,
  `var/lib/dnf`
- `var/cache`, `var/log`, `var/lib/systemd/random-seed`, `etc/machine-id`
- `etc/pki/ca-trust/extracted/java/cacerts`: a JKS trust store that
  `update-ca-trust` regenerates with fresh salt on every run. The PEM bundles
  beside it remain covered.

The installed package set is verified separately against the lock, so excluding
the rpmdb does not weaken the closure check. Special files and xattrs are not
represented.

Two independent assemblies of the same lock produced the same digest in the
Fedora 44 fixture.

An assembled root contains mode-000 files, so a non-root caller cannot read all
of it directly. `root assemble` re-enters the user namespace to compute the
digest. To run the digest manually, use
`podman unshare -- rpmx root digest <dir>`.

## 9. Consuming the root

`rpmx rebuild --root-lock root.lock.yaml --root ./root` is the only path that
uses an assembled root:

1. Load the lock and recompute `closure_digest`.
2. Require `root_digest`, and check the lock's architecture against `--arch`.
3. Recompute the tree digest and compare it with `root_digest` before running
   anything (`RPMX-ROOT-017` on mismatch).
4. Run the build steps with bubblewrap, binding the root read-only at `/`:
   `--ro-bind <root> /`, with `/dev` and `/proc` mounted, `/tmp` as a tmpfs,
   and the workspace and stage bound at `/tmp/build` and `/tmp/stage`.
   Network is unshared.

A root with only `bash` and no `coreutils` fails the fixture build with
`mkdir: command not found`, which is the proof that the steps run inside the
provided root rather than the host.

`examples/c-hello` exercises a real toolchain from the locked root: python3
writes a header, make invokes gcc, and coreutils installs the binary. The
resulting RPM installs and runs in Fedora 44. A release rpmx rebuilds it in
under two seconds, and a debug and a release build produced byte-identical
RPMs.

The root lock can be embedded in the SRPM (`rpmx srpm --root-lock`). `rebuild`
then uses the embedded lock by default, and an explicit `--root-lock` must match
its `closure_digest`. The SRPM still carries no root payloads, so the assembled
directory is supplied with `--root` (or rebuilt from the lock and the CAS).

`--root-image` remains the image-based alternative; it is mutually exclusive
with a root lock.

## 10. Document format and libpkgmanifest

The `repositories` and `packages` vocabulary is the libpkgmanifest
`rpm-package-manifest` vocabulary, used verbatim. The profile uses the
`rpm-package-input` vocabulary (`repositories`, `packages.install`,
`modules.enable`/`disable`, `archs`).

rpmx keeps its own document header and integrity fields. libpkgmanifest has no
place for `closure_digest`, `root_digest`, the builder identity, the profile
name or resolve policy, and its parser ignores unknown keys. A lock dressed as
`rpm-package-manifest` could therefore be read and re-serialised by a consumer
that silently drops the integrity fields.

Interoperability is at the boundary:

- `rpmx root export` writes a strict `rpm-package-manifest` `0.2.3` document.
- `rpmx root import` reads one back and recomputes the closure digest.
- CI parses the exported document with `python3-libpkgmanifest`, re-serialises
  it, and checks that the package set is unchanged.

rpmx does not link libpkgmanifest. It has no C API (a C++ ABI plus SWIG Python
bindings), so Rust would need a hand-written C shim for an object graph with
reference semantics; export through that graph is the expensive half. Upstream
also uses `yaml-cpp` (YAML 1.1-flavoured) while rpmx requires strict YAML 1.2,
it silently ignores unknown fields, its schema is pre-1.0, and lock durability
must not depend on a library upgrade. Revisit this if upstream grows resolve,
download or install support, gains a stable C API, or if rpmx must tolerantly
consume arbitrary third-party manifests.

## 11. Open questions

1. Scriptlet nondeterminism: the 42-package fixture exposed a JKS trust store
   regenerated with fresh salt, now excluded. A package whose scriptlet writes
   timestamps or random data into another covered path will still change the
   digest. Is that a failure or expected evidence, and should exclusions be
   profile-controlled?
2. Repo metadata snapshots: the lock records repository identity but not a
   metadata digest or content-addressed metadata snapshot. Resolve-time metadata
   is therefore not pinned, even though the closure is.
3. Module streams: profiles reject non-empty `modules` until the lock has a
   place for the module and stream.
4. SELinux labels and xattrs: not covered by `root_digest`.
5. Cross-arch: build steps use a full qemu-system VM; root assembly and
   test-root installation use podman `--arch` through host binfmt.
6. Repository keys: resolve uses `gpgcheck=1` when a `gpgkey` is declared.
   Local-payload assembly relies on CAS checksums, not OpenPGP.
7. Root cleanup: an assembled root contains mode-000 and non-writable
   directories, so removing it needs `podman unshare -- rm -rf`. rpmx does this
   for its own failed transactions but does not offer `--force` yet.
