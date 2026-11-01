# Third-party notices

`rpmx` is dual-licensed under Apache-2.0 or MIT. The full texts are in
[`LICENSE-APACHE`](LICENSE-APACHE) and [`LICENSE-MIT`](LICENSE-MIT).

## Vendored rpm-rs

`vendor/rpm` is a copy of the Rust [`rpm`](https://github.com/rpm-rs/rpm) crate,
version 0.30.2, used through the workspace `[patch.crates-io]` entry. It is
licensed Apache-2.0 OR MIT; its license files are preserved at
`vendor/rpm/LICENSE-APACHE` and `vendor/rpm/LICENSE-MIT`. The upstream authors
are listed in `vendor/rpm/Cargo.toml`.

Local modifications are limited to source-package support used by the native
SRPM writer: an explicit `source_package` builder flag sets `SOURCEPACKAGE=1`
and omits `SOURCERPM` (see `vendor/rpm/src/rpm/builder.rs`). Redistribution of
`vendor/rpm` remains under the upstream dual license.

`spikes/01-native-srpm/vendor/rpm` is a second, independent copy used only by
that historical spike. It carries an earlier `arch == src` variant of the same
patch; the delta is recorded in
`spikes/01-native-srpm/patches/0001-rpm-rs-source-package.patch`. It is not
linked into the `rpmx` binary and is not part of the workspace build.

### Test signing keys

The upstream crate ships public test keys under
`vendor/rpm/tests/assets/signing_keys` and
`spikes/01-native-srpm/vendor/rpm/tests/assets/signing_keys`. They are part of
the upstream test fixtures, are not credentials, and must not be used for real
signing. The repository's `.gitleaks.toml` allowlists those paths so secret
scanners do not flag them.

## Rust dependencies

The exact dependency graph is pinned in [`Cargo.lock`](Cargo.lock). The
licenses reported by `cargo metadata` are all permissive and compatible with
Apache-2.0 OR MIT. Summary:

| License expression | Packages |
|---|---|
| MIT OR Apache-2.0 | 158 |
| Apache-2.0 OR MIT | 41 |
| MIT | 29 |
| MIT/Apache-2.0 | 11 |
| BSD-3-Clause | 8 |
| Unlicense OR MIT | 4 |
| BSD-2-Clause OR Apache-2.0 OR MIT | 4 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 3 |
| ISC | 2 |
| BSD-3-Clause OR MIT OR Apache-2.0 | 2 |
| Zlib | 1 |
| MIT OR Zlib OR Apache-2.0 | 1 |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later | 1 |
| MIT OR Apache-2.0 OR BSD-1-Clause | 1 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 1 |
| CDLA-Permissive-2.0 | 1 |
| bzip2-1.0.6 | 1 |
| Apache-2.0 OR ISC OR MIT | 1 |
| Apache-2.0 OR BSL-1.0 | 1 |
| Apache-2.0 / MIT | 1 |
| Apache-2.0 AND ISC | 1 |
| 0BSD OR MIT OR Apache-2.0 | 1 |

`r-efi` offers `LGPL-2.1-or-later` as one of three alternatives; rpmx uses it
under MIT or Apache-2.0. `webpki-roots` is CDLA-Permissive-2.0 (data), and
`libbz2-rs-sys` is bzip2-1.0.6. Neither imposes a copyleft obligation on rpmx.

Dependency license texts are distributed with the crates in the Cargo registry
and in the dependency sources used to build a binary. When distributing a
binary, include the license texts of the crates actually linked; `cargo
license` or `cargo deny` can produce that bundle.

## Documentation and examples

The design and UX documents quote command output from Melange and other tools
for comparison. Those quotations are factual and do not incorporate their
source code. `examples/curl/curl.spec` is a comparison artifact authored for
rpmx, not a copied distribution spec.
