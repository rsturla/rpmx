# eternal-release

A minimal `fedora-release` replacement for a fictional distribution called
Eternal Linux. It ships only the files a package manager and the rest of the
system need to identify the distro and find its repositories.

## What it installs

| Path | Purpose |
|---|---|
| `/etc/os-release` | The primary distro identity (`ID=eternal`, `VERSION_ID="1"`) |
| `/etc/system-release` | Legacy one-line release file |
| `/etc/redhat-release` | Compatibility file for software that still reads it |
| `/etc/yum.repos.d/eternal.repo` | The main Eternal Linux repository |
| `/etc/yum.repos.d/eternal-updates.repo` | The updates repository |
| `/usr/share/licenses/eternal-release/LICENSE` | MIT license text |

The five release files are marked `%config(noreplace)`.

The package provides the virtual names a distro is expected to satisfy:

```
system-release
system-release(releasever) = 1
```

It conflicts with other release packages so two distro identities cannot be
installed at once:

```
fedora-release
generic-release
redhat-release
```

## Build and test

The example uses the shared Fedora build root, so it resolves without extra
flags:

```console
./target/release/rpmx lint examples/eternal-release/package.yaml
./target/release/rpmx build examples/eternal-release/package.yaml
./target/release/rpmx test examples/eternal-release/package.yaml
```

The standalone candidate-payload test checks the release identity and both repository files in
the staged source tree:

```console
grep -q '^ID=eternal$' os-release
grep -q '^VERSION_ID="1"$' os-release
test -f eternal.repo
test -f eternal-updates.repo
```

An installed-stage test is not used here: the package conflicts with
`fedora-release`, and the shared Fedora test root already contains it. A real
test root for this package would not contain another release package.

## Separate files

The payload lives in [`files/`](files/) next to the recipe, not inside a
heredoc and not as one source per file:

```yaml
context: files
```

The contents of `files/` are copied to the source workspace root, so the build
reads `os-release`, `system-release`, `redhat-release`, `eternal.repo`,
`eternal-updates.repo` and `LICENSE` directly. The build pipeline installs them
into the staging tree.

`rpmx lock` records one context tree digest, covering every path, mode and file
content. Changing any file changes the digest, and `--locked` catches it. The
files are included in the deterministic source snapshot and the SRPM, so
`rpmx rebuild` does not need the `files/` directory.

`include:` is a different feature: it splits the recipe YAML into fragments, as
[`examples/jq`](../jq/) does. It does not ship payload files.

## Example content

`/etc/os-release`:

```text
NAME="Eternal Linux"
VERSION="1 (Eternal)"
ID=eternal
ID_LIKE="rhel fedora"
VERSION_ID="1"
PRETTY_NAME="Eternal Linux 1"
ANSI_COLOR="0;31"
CPE_NAME="cpe:/o:eternal:eternal:1"
HOME_URL="https://eternal.example.org/"
DOCUMENTATION_URL="https://docs.eternal.example.org/"
SUPPORT_URL="https://eternal.example.org/support"
BUG_REPORT_URL="https://eternal.example.org/bugs"
VARIANT="Server"
VARIANT_ID=server
```

`/etc/yum.repos.d/eternal.repo`:

```ini
[eternal]
name=Eternal Linux $releasever - $basearch
baseurl=https://repo.eternal.example.org/eternal/$releasever/$basearch
enabled=1
gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-eternal
metadata_expire=7d
countme=1
```

## Notes

- The GPG key referenced by `gpgkey=` is not part of this package. Like Fedora,
  a separate key package (for example `eternal-gpg-keys`) would own
  `/etc/pki/rpm-gpg/RPM-GPG-KEY-eternal`. For a local toy repository, set
  `gpgcheck=0` in the `.repo` files.
- `/etc/redhat-release` is a regular file here for simplicity. A real distro
  often makes it a symlink to `/etc/system-release`.
- The release files are ordinary files under [`files/`](files/). The context
  directory is copied into the deterministic source snapshot and recorded in
  the SRPM. There is no upstream tarball.
