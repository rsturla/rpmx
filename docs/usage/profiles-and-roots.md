# Profiles and build roots

## Profiles

A profile is root policy, not a recipe:

```yaml
document: rpmx-profile
version: 1
name: fedora-44
releasever: "44"
dist-tag: .fc44
archs: [x86_64]
builder:
  image: registry.fedoraproject.org/fedora:44
repositories:
  - id: fedora
    metalink: https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever&arch=$basearch
  - id: updates
    metalink: https://mirrors.fedoraproject.org/metalink?repo=updates-released-f$releasever&arch=$basearch
packages:
  install: [bash, coreutils, gcc, make]
  reinstall: []
options:
  weak_deps: false
  docs: false
debuginfo: false
sbom: true
```

A repository sets exactly one of `baseurl`, `metalink`, `mirrorlist`, plus an
optional `gpgkey`. `dist-tag` is appended to every release. `debuginfo` and
`sbom` are recorded in the root lock and applied at build time. Module streams
and `reinstall` are rejected today.

### Local repositories

`baseurl` may point at a host directory:

```yaml
repositories:
  - id: local
    baseurl: file:///srv/rpmx/packages
    gpgkey: file:///srv/rpmx/keys/repo.asc
```

rpmx mounts the directory read-only into the resolver container and rewrites
the URL inside the container. A local `gpgkey` is copied into the resolver work
directory. This lets a package built locally satisfy another package's build
dependency:

```console
rpmx repo --rpms out/ --output /srv/rpmx/packages
# profile has local + distro repositories, and package A is in `packages.install`
rpmx build B/package.yaml
```

The path must exist on the resolve host and be absolute. The resulting lock
records the original repository id; for another machine, use `--root-lock` and
`root assemble` from the CAS, or publish the repository over HTTPS.

---

## Roots

### Automatic root (normal use)

With `build.root` or `--profile`, `build`, `test`, `source` and `lock` do this
automatically:

1. Merge the profile packages with the plan's dependencies and step needs.
2. Hash the requirement.
3. Look under `$XDG_CACHE_HOME/rpmx/roots/<requirement-digest>/`.
4. On a miss, resolve with dnf5, assemble offline, verify the digest and record
   `root.lock.yaml`.
5. Reuse the root on the next build.

`--offline` never resolves or assembles; it fails with the missing digest.
`--cache DIR` changes the CAS (roots live beside it).

### Explicit root

For debugging or a prebuilt root:

```console
rpmx root resolve profile.yaml --output root.lock.yaml
rpmx root assemble root.lock.yaml --root ./root --output root.assembled.yaml
rpmx build package.yaml --root-lock root.assembled.yaml --root ./root
```

Explicit roots are verified by hashing the tree. Auto-resolved roots are
trusted on a cache hit, with a mtime check; use explicit `--root` when you need
proof. `rpmx root digest ROOT` computes the deterministic digest. On a root
with mode-000 files it re-enters a user namespace via `unshare` or `podman
unshare`.

`root export` and `root import` translate a lock to and from a libpkgmanifest
`rpm-package-manifest`.

---
