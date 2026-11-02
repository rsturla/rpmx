# Changelog

All notable changes to `rpmx` are recorded here. The project is pre-1.0 and
does not promise a stable schema or CLI yet.

## Unreleased

- Foreign builds default to bubblewrap + host binfmt. `auto` and `--runner
  bwrap` run native steps directly and foreign steps through an enabled
  `F`-flag qemu-user handler; `--runner qemu` remains the explicit full-VM
  path. A missing handler fails before any step with `RPMX-RUNNER-002`.
- The build-root requirement is runner-independent. A QEMU appliance is a
  sidecar `appliance.lock.yaml` beside the cached root, not a `runner-assets`
  section in `root.lock.yaml`; `--locked` and `--runner qemu` share the same
  root and cannot drift because of the runner.
- Profiles may list several target architectures as an allowed set;
  `Profile::for_target` derives one root per target. `rpmx root resolve` gains
  `--arch`.
- Human progress output is now a leveled `INFO`/`WARN`/`ERRO` log on stderr,
  with a plan header, a resolved root block (builder and repositories), numbered
  steps, per-phase durations, relative artifact paths and a final artifact
  count. Every step line and guest line carries the active stage (`source`,
  `build`, `test`). Guest stdout and stderr are prefixed and written to stderr
  too, so stdout is only the artifact list or the single JSON document.
  `--verbose` lists the resolved root package set and step durations, and
  `--color` controls ANSI color.
- QEMU builds no longer boot the build root. A separate, locked transport
  appliance is resolved from the profile repositories, assembled into its own
  cache, and attached as a second disk. Build steps run on a chrooted build
  channel while an unchrooted control channel mounts the disk and moves the
  workspace and stage. Profiles no longer list kernel, dracut, networkd or SSH
  packages; the build root keeps its own closure. The appliance lock is a
  sidecar beside the cached root and installed test roots no longer install the
  boot set. `rpmx runner resolve|assemble|inspect` handles appliances
  independently (`rpmx appliance` remains an alias), and `--runner-asset-root`
  lets `build`, `source`, `test` and `rebuild` use a pre-assembled asset.
  `rpmx doctor` reports all runner tools and cached assets through the runner
  registry.
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
- A recipe `context:` directory copied into the source workspace, with one tree
  digest in the lock and inclusion in the source snapshot and SRPM.
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
- QEMU workspace transfers stream over ssh instead of buffering whole archives;
  step output streams live, guest stderr retains only a bounded tail, and the
  serial log is capped.
- Untrusted decompressed payloads stream to disk instead of host memory, with
  no arbitrary SRPM/RPM or download size ceiling. Structural limits cover tar
  extension headers, sparse entries, bundle metadata and decompressed kernel
  images; guest sync-out keeps an 8 GiB cap because it is attacker output.
- `--jobs` and `--step-timeout` require a positive value; a step timeout can no
  longer be disabled.
