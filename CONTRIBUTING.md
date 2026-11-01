# Contributing

`rpmx` is an early prototype. Small, focused changes with evidence are much
more useful than broad rewrites.

User-facing instructions live in [`docs/usage/`](docs/usage/). The design
study, build model, sandbox contract and UX audit live in
[`docs/contributing/`](docs/contributing/). Read the relevant design document
before changing behaviour it describes.

## Build and test

```console
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

The acceptance harnesses need `podman` and are run manually:

```console
scripts/dnf-corpus-test.sh <rpm-dir> <upgrade-dir>
scripts/replacement-dnf-test.sh
scripts/systemd-dnf-test.sh <v1-rpm-dir> <v2-rpm-dir>
```

## Design rules

- Read the surrounding code and match its structure and naming.
- The smallest viable diff. Do not refactor adjacent code as a side effect.
- Every bug fix needs a regression test that fails without the fix.
- Do not add a dependency or an abstraction for a single caller.
- Keep the spec and the implementation in sync. If they disagree, fix one in
  the same change.

## Commits

One logical change per commit. Say what changed and why. Do not commit build
output, RPMs, caches, or generated SBOMs.

## Licensing

rpmx is dual-licensed under Apache-2.0 or MIT. By submitting a change you agree
that it may be distributed under those terms. Do not add code, test assets or
documentation that cannot be distributed under them. Preserve the license and
attribution of vendored code; see [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

The CI workflow runs formatting, clippy and the test suite. Before a release,
also run `cargo deny check licenses advisories` and review
[`RELEASING.md`](RELEASING.md).
