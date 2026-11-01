# Changelog

All notable changes to `rpmx` are recorded here. The project is pre-1.0 and
does not promise a stable schema or CLI yet.

## Unreleased

- Steps no longer hardcode guest paths. Built-in steps use `$STAGE`, `$PREFIX`,
  `$LIBDIR` and `$JOBS` (or the `${...}` placeholders); the planner keeps those
  placeholders in the plan, lock and SRPM, and the loader rejects a literal
  guest path with `RPMX-STEP-019`. Profiles gain `paths.prefix` and
  `paths.libdir`, recorded in the root lock.
- File-payload dependency generators: ELF sonames, symbol versions and `.so`
  symlink requires; pkgconfig provides and requires; shebang interpreters;
  Python ABI and `pythonX.Ydist(...)`; Perl modules; desktop/AppStream
  provides; and debuginfo build-ids.
- Per-output `generators:` switches and exact-match `exclude` lists.
- Native SRPM and binary RPM writers, with `rpmbuild` never invoked.
- Weak runtime dependencies (`recommends`, `suggests`, `supplements`,
  `enhances`) in recipes, RPM headers and repository metadata.
- Locked build roots resolved with dnf5 and assembled offline from the CAS.
- Source snapshots, patches, git sources and a content-addressed cache.
- Matrix and template planning, reusable steps, build and installed tests.
- SPDX 2.3 SBOMs, CPE references and optional OpenPGP signing.
- File directories, per-path owner/group/mode and file capabilities.
- `rpmx repo` repository metadata and signed `repomd.xml`.
- One-command `rpmx build` from recipe to SRPM to binary RPMs.
- Host/target architecture separation and native-architecture defaults.
- A session-based `Runner` interface with bubblewrap, podman and qemu-system
  backends. Foreign builds use a full VM with a host-only SSH control NIC and
  tar-based workspace transfer; root assembly and test-root installation use
  podman --arch. Root assembly, SRPM embedding and installed tests follow the
  same host/target contract.
