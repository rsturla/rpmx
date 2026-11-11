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

On an SELinux enforcing host, test concurrent resolver repository mounts with:

```console
cargo test -p rpmx-root concurrent_resolvers_can_read_the_same_local_repository -- --ignored
```

The test uses Fedora 44 by default. Set `RPMX_TEST_IMAGE` to use another image
with `sh`, `cat`, `touch`, `seq` and `sleep`. Local repositories and cached RPMs
use shared SELinux labels because concurrent root preparations read the same files.

To check builder image pins with Podman and access to the Fedora registry, run:

```console
cargo test -p rpmx-root an_oci_index_and_its_platform_manifests_resolve_to_the_same_locked_bytes -- --ignored
cargo test -p rpmx-root a_locked_platform_image_runs_when_its_original_tag_no_longer_exists -- --ignored
```

These tests check both platform selections, explicit manifest pins, wrong
architectures and a missing original tag. Image locks record the selected
platform manifest digest. Podman's image `Digest` field can identify its parent
index, so resolution uses temporary manifest metadata instead. No image layers
are downloaded to select a manifest.

To exercise package acceptance with local RPM repositories, a cached Fedora 44
builder image, Podman and configured bubblewrap namespaces, run:

```console
cargo test -p rpmx-application --test acceptance package_tests_resolve_only_required_outputs_and_replay_exact_environments -- --ignored
```

This regression checks candidate selection, minimal output closures, immutable
fixtures, private transaction state and recipe-bound replay through the public
application interface. It generates repository payloads locally.

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

The CI workflow runs formatting, clippy, the test suite and
`cargo deny check licenses`. Advisory findings are reported but are not
merge-blocking; triage them separately. Before a release, also run
`cargo deny check advisories` and review [`RELEASING.md`](RELEASING.md).
