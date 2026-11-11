# Profiles and build roots

Root resolution reuses public remote repository metadata from
`<CAS>/dnf-metadata-v1`. Each resolver gets private copies and refreshes
`repomd.xml`; changed repositories still produce a fresh package resolution.
Local repositories, authenticated repositories, downloaded RPMs and previous
transactions are not shared through this metadata cache. Corrupt snapshots
cause a cold resolution and are repaired after a successful download.

Resolution prefers the newest compatible versions within repository policy.
A generic package request can select an older version when another package
requires it exactly. Exact requests and all package dependencies remain
mandatory. An unsatisfiable profile fails; the resolver does not skip broken
packages to produce a partial root. This policy is explicit and does not depend
on the builder image's DNF defaults.

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
paths:
  prefix: /usr
  libdir: /usr/lib64
  libdir-by-arch:
    aarch64: /usr/lib
sbom: true
```

A repository sets exactly one of `baseurl`, `metalink`, `mirrorlist`, plus an
optional `gpgkey`. `dist-tag` is appended to every release. `sbom` is
recorded in the root lock and applied at build time. `paths.prefix`
and `paths.libdir` set the install layout: the executor exports them as
`PREFIX` and `LIBDIR`, substitutes them for `${prefix}` and `${libdir}`, and
records them in the root lock. `libdir-by-arch` overrides `libdir` for the
listed target architectures; keys use canonical RPM architecture names. Each
library directory must be inside `prefix`. Module streams and `reinstall` are
rejected today.

A profile does not contain compiler or linker flags. Distribution build
policy belongs in a package in the root: the package installs compiler specs
or wrapper scripts, and every build that installs it inherits that policy.
This keeps the profile a description of *where packages come from and which
ones are installed*, not a second place where hardening or optimisation is
configured. Cross-only toolchain quirks still live in `cross.cflags`,
`cross.cxxflags` and `cross.ldflags`.

### Local repositories

`baseurl` may point at a host directory. A `file://` path may be absolute, or
relative to the profile file; rpmx resolves it when it loads the profile:

```yaml
repositories:
  - id: local
    baseurl: file:///srv/rpmx/packages
    gpgkey: file:///srv/rpmx/keys/repo.asc
  - id: toolchain
    baseurl: file://../.work/toolchain-repo
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

### Authenticated repositories

A repository that needs a login can authenticate with a TLS client certificate
(RHEL CDN), basic auth (Satellite or a private mirror), or a proxy. The profile
holds only references, never the secret itself:

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

Basic auth and proxy credentials use the same block:

```yaml
    auth:
      username: builder
      password: env:RHEL_PASSWORD
      proxy: http://proxy.example:3128
      proxy-username: builder
      proxy-password: env:PROXY_PASSWORD
```

Rules:

- `ca`, `cert`, `key`, `password` and `proxy-password` take an `env:NAME`
  reference or an absolute path. `file:///absolute/path` is also accepted.
  Passwords must not be literals.
- `cert` and `key` are set together; `username` and `password` are set together;
  `proxy-username` and `proxy-password` are set together.
- Credentials are honoured only in a profile the user controls: one passed with
  `--profile FILE` or found under `$XDG_CONFIG_HOME/rpmx/profiles`. A profile
  selected by the recipe itself cannot carry `auth`; pass the profile explicitly
  or move it into the config directory.
- Do not put `user:password@` in a repository or proxy URL. Use the `auth` block.
- Credential files are copied into a private temporary directory, mounted
  read-only into the resolver container, and removed when resolution finishes.
  The build sandbox still has no network and no credentials.
- `repo-gpgcheck: true` verifies signed repository metadata as well as package
  signatures. RHEL repositories usually need it.

Resolve normally:

```console
RHEL_CLIENT_CERT=... RHEL_CLIENT_KEY=... \
  rpmx root resolve profiles/rhel-9.yaml --output root.lock.yaml
```

The lock and the SRPM do not contain the `auth` block. `root assemble` and
`rebuild` use the content-addressed cache, so they do not need the credentials.
A host without entitlement can build from a cache populated by an entitled
host. Authentication problems are reported as `RPMX-ROOT-019`.

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

`--profile FILE` wins over a profile the recipe names in `build.root`, and its
policy (dist tag) follows the same profile. An inline `build.root` cannot be
combined with `--profile`; remove one.

`--offline` never resolves or assembles; it fails with the missing digest.
`--cache DIR` changes the CAS (roots live beside it).

Root assembly uses the builder's platform manifest digest recorded in the lock.
A moving image tag does not change an existing lock. The pinned image must
remain available locally or in its registry, and its architecture must match
the locked root.

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

### QEMU appliance

A QEMU build does not boot the build root. It boots a separate transport
appliance containing the kernel, initramfs, systemd-networkd, `openssh-server`
and `tar`. The appliance is resolved from the profile's repositories, but its
package list is owned by rpmx; do not add those packages to
`packages.install`.

The appliance is a runner asset, not part of the root lock. A QEMU build
resolves and assembles it on demand and caches it under
`$XDG_CACHE_HOME/rpmx/appliances/<identity>/root`. Use
`rpmx runner resolve|assemble|inspect` to manage it explicitly, or pass
`--runner-asset-root DIR` to `build`, `source`, `test` or `rebuild` to use a
preassembled one.

`--offline` never resolves an appliance; an uncached one is
`RPMX-ROOT-020`.

The appliance can also be handled on its own:

```console
rpmx runner resolve profiles/fedora-44.yaml --output appliance.yaml
rpmx runner assemble appliance.yaml --root ./appliance
rpmx runner inspect --asset-lock appliance.yaml
rpmx build package.yaml --runner qemu --runner-asset-root ./appliance
```

`--runner-asset-root` bypasses the appliance cache for `build`, `source`, `test`
and `rebuild`; the directory must already contain a bootable appliance.

`root export` and `root import` translate a lock to and from a libpkgmanifest
`rpm-package-manifest`.

---
