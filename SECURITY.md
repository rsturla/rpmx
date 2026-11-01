# Security policy

`rpmx` is pre-release software. It is not yet supported for production use and
has no long-term security support window.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately
through the repository's security advisory feature, or contact the maintainers
through the same private channel used for the repository.

Include:

- the rpmx version (`rpmx --version`) and commit;
- the host distribution, kernel and Rust version;
- the recipe, profile and lock needed to reproduce, with secrets removed;
- the affected trust boundary: source fetching, SRPM extraction, root
  resolution, sandbox escape, path traversal, signature or digest handling,
  cache poisoning, or key handling;
- the observed and expected behaviour.

We will acknowledge the report, confirm the affected versions, and coordinate a
fix and disclosure. Please give us a reasonable window before publishing
details.

## Scope

The threat model is documented in
[`docs/contributing/08-threat-model.md`](docs/contributing/08-threat-model.md).
The most relevant boundaries are:

- the acquisition proxy is untrusted; source bytes are verified against the
  lock before use;
- an SRPM is untrusted until its manifest and digests are verified;
- the build sandbox is assumed to contain hostile build input;
- source archives and RPM payloads are untrusted parsers;
- rpmx never stores, discovers or logs signing keys or passphrases.

## Test fixtures

`vendor/rpm/tests/assets/signing_keys` and
`spikes/01-native-srpm/vendor/rpm/tests/assets/signing_keys` contain upstream
`rpm-rs` test keys. They are public test fixtures, not credentials, and must not
be used to sign real packages. See [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
