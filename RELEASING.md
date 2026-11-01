# Releasing rpmx

rpmx is pre-1.0. A release is a source archive, a built binary and, when ready,
an OCI image. The crates are not currently published to crates.io because the
build depends on a locally patched `rpm` crate (see below).

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

5. Verify packaging contents without uploading:

```console
cargo package --list -p rpmx
```

6. Build and smoke-test the release binary:

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

## Why crates.io is not used yet

The workspace patches crates.io `rpm` 0.30.2 to a local copy:

```toml
[patch.crates-io]
rpm = { path = "vendor/rpm" }
```

The patch adds native source-package support used by the SRPM writer. A crate
published to crates.io cannot carry that patch; consumers would resolve the
unpatched upstream crate. Publishing the workspace therefore requires one of:

1. upstreaming the source-package patch to `rpm-rs`;
2. publishing a maintained fork under a distinct crate name; or
3. removing the dependency on the patched crate.

Until then, release source archives and binaries, not crates.io packages.
`cargo package --list` is still useful to verify the file list.

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
