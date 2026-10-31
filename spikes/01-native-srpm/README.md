# Spike 01: a native SRPM without a spec

## Question

Can a native `rpmx` source package be written without a spec file, and will stock
RPM accept it?

This is the critical-path question. The design says native SRPMs have no
`SPECS/`, are rebuilt only by `rpmx`, and still must be ordinary RPM files that
`rpm`, `rpm2cpio` and repository tooling can read.

## Environment

| Tool | Version |
|---|---|
| RPM | 6.0.2 |
| `rpmbuild` | not installed (deliberately) |
| Rust / cargo | 1.99.1 |
| `rpm` crate | 0.30.2 |

## What was built

`src/main.rs` uses `rpm::PackageBuilder` to write a source package containing:

- `package.yaml`
- `package.lock.json`
- `RPMX-MANIFEST.json`

The payload is a normal cpio archive; no spec is generated or embedded.

The crate cannot mark a package as a source package and sets `SOURCERPM` on
every package. Both are wrong for an SRPM. `patches/0001-rpm-rs-source-package.patch`
is a 12-line patch:

- set `SOURCEPACKAGE=1` when the package is a source package;
- omit `SOURCERPM`, which is how stock RPM distinguishes binary from source
  packages.

## Results

```
$ cargo run --release -- demo.src.rpm
$ rpm -qp --qf 'NAME=%{NAME} ARCH=%{ARCH} SOURCEPACKAGE=%{SOURCEPACKAGE}\n' demo.src.rpm
NAME=rpmx-demo ARCH=src SOURCEPACKAGE=1

$ rpm -Kv demo.src.rpm
demo.src.rpm:
    Header SHA256 digest: OK
    Payload SHA256 digest: OK

$ rpm2cpio demo.src.rpm | cpio -it
./RPMX-MANIFEST.json
./package.lock.json
./package.yaml
```

Determinism: two runs with the same `source_date` are byte-identical.

```
7f5be2dd...  a.src.rpm
7f5be2dd...  b.src.rpm
```

For comparison, a Fedora 44 source RPM (`hello-2.12.3-1.fc44.src.rpm`) has
`SOURCEPACKAGE=1`, no `SOURCERPM`, a flat file list (`hello.spec`,
`hello-2.12.3.tar.gz`, ...), and `PAYLOADCOMPRESSOR=gzip`.

## Findings

| Question | Result |
|---|---|
| Is a spec required? | No. `rpm` reads the package, verifies digests and extracts the payload. |
| Does the `rpm` crate work? | Almost. Binary packages are fine; source packages need the two tag fixes. |
| Is the output deterministic? | Yes, with a fixed `source_date` and sorted inputs. |
| Does stock RPM classify it as source? | Yes, with `SOURCEPACKAGE=1` and no `SOURCERPM`. |
| Does `rpm -i` install it? | It refuses with `source package contains no .spec file`, which is expected and irrelevant. SRPMs are extracted, not installed. |

## Gaps this exposes

1. **`ARCH` is `src`.** Real SRPMs use the build architecture (`i686`, `x86_64`)
   and mark source with `SOURCEPACKAGE=1`. The production writer must not derive
   "source" from the arch string; it needs an explicit source flag.
2. **File list paths are absolute.** The crate writes `/package.yaml` in the
   header while the cpio entry is `./package.yaml`. Real SRPMs write flat names
   (`hello.spec`). This is cosmetic for `rpmx` rebuilds but should be fixed for
   interoperability.
3. **Payload compressor is zstd.** Valid for RPM 4.14+, but source packages
   conventionally use gzip. Make it configurable and default to gzip for SRPMs.
4. **No signature.** Expected: `rpmx` does not sign. The package verifies as
   unsigned.
5. **The crate is patched, not vendored upstream.** A production implementation
   either upstreams the source-package support, carries a small patch, or writes
   the source header directly.

## Verdict

The native SRPM is feasible now. The cheapest path is a small patch to `rpm-rs`
plus a thin `rpmx` writer layer that sets the build arch, normalises payload
paths and chooses compression. It does not require writing an RPM header
serializer from scratch.

The remaining risk is the **binary** writer, not the source writer: scriptlets,
file metadata, dependency generation and triggers have no `rpmbuild` fallback.

## Reproduce

```sh
cd spikes/01-native-srpm
cargo run --release -- demo.src.rpm
./verify.sh
```

`vendor/rpm` is the patched crate; `patches/0001-rpm-rs-source-package.patch`
shows the delta against `rpm` 0.30.2.
