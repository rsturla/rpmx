# Changelog

All notable changes to `rpmx` are recorded here. The project is pre-1.0 and
does not promise a stable schema or CLI yet.

## Unreleased

- Native SRPM and binary RPM writers, with `rpmbuild` never invoked.
- Locked build roots resolved with dnf5 and assembled offline from the CAS.
- Source snapshots, patches, git sources and a content-addressed cache.
- Matrix and template planning, reusable steps, build and installed tests.
- SPDX 2.3 SBOMs, CPE references and optional OpenPGP signing.
- File directories, per-path owner/group/mode and file capabilities.
- `rpmx repo` repository metadata and signed `repomd.xml`.
- One-command `rpmx build` from recipe to SRPM to binary RPMs.
- Host/target architecture separation and native-architecture defaults.
- A `Runner` interface with native bubblewrap/container runners and a
  qemu-user binfmt emulated runner for foreign targets. Root assembly, SRPM
  embedding and installed tests follow the same host/target contract.
