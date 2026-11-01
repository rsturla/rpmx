# Releasing rpmx

rpmx is pre-1.0. A release is a source archive, a built binary and, when ready,
an OCI image. rpmx is distributed as those artifacts; it is not published to
any package registry.

## Before tagging

1. Pick the version and update it in exactly one place, `[workspace.package]` in
   `Cargo.toml`. All crates inherit it.
2. Update `CHANGELOG.md`; move `Unreleased` entries under the version and date.
3. Check that the docs still match the CLI:
   - `docs/usage/` for user-facing commands and schema;
   - `docs/contributing/` for the design and UX audit.
4. Run the full gate:

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses advisories
gitleaks detect --no-git -c .gitleaks.toml
```

If `cargo deny` or `gitleaks` is not installed, install them or record the
exception in the release notes. Do not release with a failing license check.

5. Build and smoke-test the release binary:

```console
cargo build --release
./target/release/rpmx --version
./target/release/rpmx doctor
./target/release/rpmx init /tmp/rpmx-release-smoke
cd /tmp/rpmx-release-smoke
/tmp/.../rpmx lint
/tmp/.../rpmx build --no-index
/tmp/.../rpmx test --install-dir packages/x86_64
```

## Source release

Create an archive from tracked files only:

```console
git archive --format=tar.gz --prefix=rpmx-<version>/ \
  -o rpmx-<version>.tar.gz HEAD
```

The archive includes `LICENSE-APACHE`, `LICENSE-MIT`, `THIRD_PARTY_NOTICES.md`,
`SECURITY.md`, `RELEASING.md`, `Cargo.lock`, the vendored `rpm` crate and the
spike copies. Do not add `target/`, `packages/`, RPMs, SBOMs or cache contents.

Sign the archive and publish a checksum beside it.

## Binary release

`cargo build --release` produces `target/release/rpmx`. Stripping is optional.
Publish the binary with the license files and `THIRD_PARTY_NOTICES.md`, or make
the source archive the primary artifact and treat binaries as convenience
builds. If the binary is distributed, include the licenses of the crates linked
into it; `cargo deny` and `cargo license` can generate that bundle.

## Vendored rpm crate

The workspace patches upstream `rpm` 0.30.2 to a local copy:

```toml
[patch.crates-io]
rpm = { path = "vendor/rpm" }
```

The patch adds native source-package support used by the SRPM writer, so the
build only works inside this repository. `vendor/rpm` must therefore ship in
every source release and must stay in sync with the patch described in
`THIRD_PARTY_NOTICES.md`. Do not replace it with the released upstream crate:
doing so silently drops the `source_package` flag and produces invalid SRPMs.

## Release notes

State:

- the supported host architectures and runners;
- the required external tools (`bubblewrap`, `podman`, `dnf5`, QEMU tooling);
- the security posture: pre-release, no production support;
- any change to the recipe schema, lock schema or CLI surface;
- known limitations from `docs/usage/reference.md`.

## Security releases

Follow [`SECURITY.md`](SECURITY.md). Prepare the fix and regression test before
public disclosure. Credit the reporter if they agree. Publish the advisory,
the fixed source archive and a signed checksum together.
