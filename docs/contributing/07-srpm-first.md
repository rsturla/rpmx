# SRPM-first builds

## 1. The mandate

**Every build produces an SRPM. Every build can be reproduced from an SRPM. The SRPM is the canonical source artifact.**

That is a stronger and simpler contract than "a recipe plus a source bundle". It means:

- `rpmx build` is internally two stages: recipe → SRPM → binary RPMs.
- `rpmx rebuild foo.src.rpm` is the same build path, starting from the SRPM.
- the SRPM contains everything needed to rebuild without network access:
  - the recipe,
  - the lock,
  - the sources,
  - the patches,
  - auxiliary files,
  - the build manifest.
- the lookaside cache, if it exists at all, is only a way to populate sources *before* the SRPM is created. It is not part of the rebuild path.

This also means the SRPM is not a `rpmbuild` compatibility artifact. It is the build system's source IR, and `rpmx` is required to rebuild it.

## 2. Pipeline

```
package.yaml
package.lock.json
patches/ files/ tests/
        |
        v
   source resolution
   (digest + locators)
        |
        v
   source assembly
   (vendor, git archive,
    deterministic generation,
    license collection)
        |
        v
   SRPM assembly
   (sources + recipe + lock
    + patches)
        |
        v
   SRPM verification
   (signature, manifest, digests)
        |
        v
   sandboxed build
   (no network)
        |
        v
   binary RPMs + SBOM/provenance
```

`rpmx build` runs the whole chain. `rpmx rebuild` starts at "SRPM verification". There is no separate source bundle and no separate "source input" path.

## 3. Native SRPM layout

A native `rpmx` SRPM is still a valid RPM source package. It contains:

```
curl-8.11.1-1.src.rpm
├── curl-8.11.1.tar.xz          # upstream source
├── package.yaml                # recipe
├── package.lock.json           # lock
├── RPMX-MANIFEST.json          # digests, plan, toolchain, provenance
├── RPMX-STEPS.json             # resolved local step definitions
├── RPMX-BUNDLED.json           # component inventory for bundled provides
└── ...                         # additional flat source files
```

There is no `SPECS/` directory. The recipe and lock are the build definition. `rpmx` is required to rebuild.

### `RPMX-MANIFEST.json`

This is the bridge between the RPM world and the typed model:

```json
{
  "schema": 1,
  "generator": "rpmx 0.0.0",
  "source_date_epoch": 1735689600,
  "recipe": { "path": "package.yaml", "digest": "sha512:..." },
  "lock": { "path": "package.lock.json", "digest": "sha512:..." },
  "sources": [
    { "path": "curl-8.11.1.tar.xz", "digest": "sha512:...", "size": 1234567 }
  ],
  "steps": [
    { "name": "acme/strip", "digest": "sha512:..." }
  ],
  "bundled": {
    "path": "RPMX-BUNDLED.json",
    "digest": "sha512:..."
  }
}
```

The manifest lets `rpmx` verify that the SRPM has not been tampered with, that the recipe and lock match, and that the source digests match the lock. Patch and toolchain entries will be added as those stages are implemented; the schema is additive there.

### 3.1 Normative payload contract

A native `rpmx` SRPM must contain exactly these classes of files. Anything else is either forbidden or explicitly marked as an extension.

| Path | Required | Purpose |
|---|---|---|
| `<upstream files>` | yes | Upstream source files, flat and with their original names. |
| `package.yaml` | yes | Recipe. |
| `package.lock.json` | yes | Lock. |
| `RPMX-MANIFEST.json` | yes | Digests, source-date-epoch, generator version. |
| `RPMX-STEPS.json` | if local steps | Map of step name to step definition, so the rebuild is offline. |
| `RPMX-BUNDLED.json` | yes | Component inventory, including explicit C/C++ declarations. Empty when nothing is bundled. |

Source files with a reserved name (`package.yaml`, `package.lock.json`,
`RPMX-MANIFEST.json`, `RPMX-STEPS.json`, `RPMX-BUNDLED.json`) are rejected. The payload is flat
because stock `rpm2cpio`/`rpm2archive` mishandle relative subdirectories in a
source package; source tarballs are flat in real SRPMs too.

Vendored dependencies and deterministic generated sources are part of the source snapshot; they are not separate payload classes. The snapshot digest and generator versions are recorded in `RPMX-MANIFEST.json`. The derived component inventory is `RPMX-BUNDLED.json`; its schema, catalogers and manual declaration rules are in [`19-bundled-provides.md`](19-bundled-provides.md).

Forbidden in the SRPM:

- binary RPMs, object files, build directories, caches, logs;
- `.git`, CI configuration or editor metadata;
- credentials, tokens, private keys or keyrings;
- absolute host paths or temporary-directory paths;
- `NoSource`/`NoPatch` omissions in a native `rpmx` SRPM;
- files that are not listed in `RPMX-MANIFEST.json`.

The header may still carry `Source`/`Patch` tags for RPM query compatibility. If present, they must match the payload exactly. They are metadata, not a build definition.

### 3.2 Header contract

The SRPM header must carry enough metadata to identify and verify the source package:

| Tag | Rule |
|---|---|
| `Name`, `Epoch`, `Version`, `Release` | Match `package` and the lock exactly. |
| `Summary`, `License`, `URL` | Match `package`; `SourceLicense` if sources differ. |
| `Vendor`, `Packager`, `Distribution` | From the profile; normalized and reproducible. |
| `Source0..N`, `Patch0..N` | One per payload file; no undeclared source. |
| `BuildRequires`, `BuildConflicts` | Resolved build dependency set, not the raw recipe. |
| `ExclusiveArch` / `ExcludeArch` | From `package.architectures`, if present. |
| `BuildHost` | Normalized (not a developer hostname). |
| `BuildTime` | Normalized to `source_date_epoch` for reproducible SRPM assembly. |
| Signature | Detached OpenPGP signature, applied by an external signer. `rpmx` never produces it and it is not part of the unsigned reproducibility comparison. |

No custom RPM header tags are required. The machine-readable metadata lives in `RPMX-MANIFEST.json`; RPM consumers see ordinary `Source`/`Patch` tags.

### 3.3 No generated spec

Native SRPMs contain no `SPECS/` directory and no spec file. The build definition is `package.yaml` + `package.lock.json`; the plan is derived from them.

Consequences:

- `rpmbuild --rebuild` cannot rebuild a native SRPM.
- `rpmspec`, `rpm -q --specfile` and workflows that assume a spec do not apply.
- `rpmx` is required to rebuild, and therefore becomes part of the bootstrap and supply-chain path.
- `rpmx` must be packaged, signed, and available in the build root or as a verified static binary.

This is intentional. The alternative is a second executable build definition that can drift from the recipe and lock.

### 3.4 Validation contract

`rpmx verify-srpm` must fail if any of the following is true:

- the signature is invalid or untrusted, when policy requires a signature;
- `Name`/`Epoch`/`Version`/`Release` disagree with the manifest;
- `package.yaml`, `package.lock.json` or `RPMX-MANIFEST.json` is missing;
- the recipe or lock digest does not match the manifest;
- a source or patch digest does not match the lock/manifest;
- a step definition in `steps/` does not match its lock entry, or a referenced step is missing;
- a `Source`/`Patch` tag, if present, has no payload file, or a payload file is not listed in the manifest;
- the SRPM contains an absolute path, `..` traversal, duplicate member, or a symlink escaping the extraction root;
- the payload contains forbidden files (binaries, caches, secrets);
- the manifest schema is unknown.

A native SRPM must be rebuildable offline with `rpmx rebuild`.

### 3.5 Native-only contract

The SRPM is native-only. There is no compatibility mode, no `--legacy-spec`, and no `rpmbuild --rebuild` path.

This makes the contract simpler:

- one build definition (recipe + lock);
- one rebuild tool (`rpmx`);
- one source artifact;
- no generated spec to validate or drift.

It also makes `rpmx` a bootstrap dependency. That has to be explicit:

- `rpmx` is packaged and signed like any other build tool;
- a static binary or a minimal bootstrap environment can be used;
- the bootstrap path is documented and audited;
- downstreams that require stock `rpmbuild` are not supported.

**Implementation note:** native SRPM writing is an early milestone, not a late optimization. M1 evaluates `rpm-rs`/`rpm` `PackageBuilder`, `librpmbuild` bindings and a minimal header+payload writer. There is no spec fallback: if none can construct a valid source package without a spec, the milestone is blocked. The binary writer is a release gate, not a later optimization. [`spikes/01-native-srpm`](../../spikes/01-native-srpm/README.md) shows `rpm` 0.30.2 works with a small tag patch; [`spikes/02-native-binary`](../../spikes/02-native-binary/README.md) shows the same for installable binary packages.

## 4. Building from an SRPM

`rpmx rebuild curl-8.11.1-1.src.rpm`:

1. **Verify the SRPM signature** against the configured policy. Unsigned SRPMs are rejected unless the profile allows them.
2. **Extract safely** with `rpm2archive`/`rpm2cpio` into a fresh directory:
   - reject absolute paths and `..` traversal,
   - reject symlinks that escape the extraction root,
   - preserve modes and ownership only where policy allows.
3. **Read `SOURCES/package.yaml` and `SOURCES/package.lock.json`**.
4. **Verify `RPMX-MANIFEST.json`** against the recipe, lock, sources and patches.
5. **Verify every source and patch digest** before use.
6. **Run the normal planner and sandboxed executor** with network disabled.
7. **Emit binary RPMs**, SBOM, provenance and a reproducibility report.

The SRPM is the only input. The build does not consult the recipe directory, the lookaside, or the network.

An SRPM that does not contain `package.yaml` and `package.lock.json` is not a native `rpmx` SRPM. `rebuild` rejects it. There is no legacy rebuild path and no automatic conversion. Existing spec-based packages are migrated with `rpmx import-spec` at the recipe level, not by rebuilding old SRPMs.

## 5. Building from a recipe

`rpmx build` is a convenience wrapper around the SRPM path:

1. Resolve features, dependencies and sources from `package.yaml` + `package.lock.json`.
2. Fetch sources from locators into the local CAS; verify digests and signatures.
3. Assemble the deterministic source snapshot (vendoring, Git archive, generation) in the source-assembly sandbox.
4. Assemble the SRPM.
5. Re-enter the build at the "verify SRPM" step.
6. Build binary RPMs.

This has a useful property: **the local build exercises the same artifact that downstreams will rebuild**. If the SRPM is broken, the local build fails too. There is no second, privileged source path that only maintainers use.

`rpmx source` stops after step 3; `rpmx srpm` stops after step 4; `rpmx build --from-srpm FILE` is equivalent to `rpmx rebuild FILE`.

## 6. Determinism

An SRPM is a tar-like payload plus a header. To make it reproducible:

- source assembly produces a deterministic snapshot: sorted paths, normalized mtimes, no host paths, stable archive order;
- all generators are pinned and their commands/versions recorded in the manifest;
- generator output is hashed; a nondeterministic generator fails source assembly unless policy marks it build-time only;
- sort the payload file list deterministically;
- normalize mtimes to `SOURCE_DATE_EPOCH`;
- normalize uid/gid to `0/0` (or the policy's build user);
- normalize modes;
- use deterministic compression settings;
- avoid embedding the build hostname, build time or builder identity in the header;
- sign outside `rpmx` and outside the reproducibility comparison (signature bytes are expected to differ).

`rpmx test --reproduce` compares the unsigned SRPM payload and header, then compares binary RPMs built from it. If two SRPMs differ only in signature, that is not a reproducibility failure.

## 7. Signing and provenance

- The SRPM is signed with the distro/repository key.
- Binary RPMs are signed.
- The provenance attestation includes the SRPM digest, the recipe/lock digests and the source digests.
- The SBOM includes the source components, licenses and digests.
- The repository stores SRPMs next to binary RPMs and serves the attestation.

## 8. What this does to the lookaside debate

With an SRPM-first model, the lookaside is no longer the durable source of truth:

- **First build of a new version**: fetch upstream sources by digest from locators (HTTPS, mirrors, OCI, local file). A lookaside can be one locator, but it is not required.
- **After the SRPM exists**: the SRPM is the durable artifact. Downstreams rebuild from it. No lookaside access is needed.
- **Archival**: store SRPMs in the repository (or an OCI/HTTP artifact store) alongside binaries. The SRPM already contains the sources.
- **Air-gapped**: copy the SRPM. No cache, no service.

This is the simplest answer to "we must use SRPM": the SRPM *is* the self-contained source store. The lookaside becomes an optional staging cache for the first fetch, not a permanent dependency.

## 9. CLI

```
rpmx fetch                      # populate CAS from locators/proxy
rpmx source                     # assemble the deterministic source snapshot
rpmx srpm                       # create an SRPM from the recipe + lock
rpmx build package.yaml         # recipe -> source -> SRPM -> binary RPMs
rpmx build --from-srpm FILE     # SRPM -> binary RPMs
rpmx rebuild FILE               # alias for build --from-srpm
rpmx inspect-srpm FILE          # show recipe/lock/manifest/sources
rpmx verify-srpm FILE           # signature + manifest + digests
```

`rpmx build --no-srpm` should not exist. If the artifact is mandatory, the tool should always produce it.

## 10. Tradeoffs

**Pros**

- One canonical source artifact.
- Downstream rebuilds need no lookaside, no recipe repo, and no network.
- Source review sees the actual bytes.
- Signing and provenance have a single artifact to bind to.
- Local builds exercise the same SRPM path as downstream builds.
- Native SRPMs have no generated spec and no second build definition.

**Cons**

- SRPMs are large; storing them next to binaries roughly doubles repository size for source-heavy packages.
- SRPM creation requires the sources; you still need a first-fetch path (upstream URL + digest + mirrors).
- Native source-package writing is early work; the format is complex.
- `rpmx` becomes a required rebuild dependency and must be bootstrapped.
- Downstreams that only have `rpmbuild` cannot rebuild native SRPMs.
- Deterministic SRPM assembly is work, especially around compression and header fields.

## 11. Open questions

1. **Should native SRPMs carry `Source` tags at all?** Optional. If the writer sets them, they must match the payload; otherwise `RPMX-MANIFEST.json` is the source of truth.
2. **How do we bootstrap `rpmx`?** A signed static binary, a bootstrap SRPM built by an older tool, or a minimal trusted environment. This must be documented and audited.
3. **How do we handle very large sources?** SRPM compression and repository storage; possibly an "SRPM without sources" mode is forbidden if SRPM is mandatory.
4. **How do we verify SRPM signatures without a keyring in the sandbox?** Verify before the sandbox, pass only the verified extraction.
5. **Should `rpmx build` always write the SRPM to the output directory, even for local iteration?** Yes if the artifact is mandatory; a `--workdir` can keep it out of the repo until requested.
6. **How do we compare an SRPM built from a recipe with one rebuilt downstream?** Compare unsigned payload/header and manifest; signatures are expected to differ.
