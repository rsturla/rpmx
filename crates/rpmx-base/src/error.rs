use std::fmt;

use serde::Serialize;

/// A stable diagnostic code such as `RPMX-STEP-011`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Code(&'static str);

impl Code {
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for Code {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0)
    }
}

impl PartialEq<&str> for Code {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// Every diagnostic code in the tool.
///
/// Names are derived from the code (`RPMX-STEP-011` is `STEP_011`) so there is
/// one obvious constant per code and no second naming convention to maintain.
pub mod codes {
    use super::Code;

    macro_rules! define_codes {
        ($($(#[$meta:meta])* $name:ident = $code:literal),* $(,)?) => {
            $(
                $(#[$meta])*
                pub const $name: Code = Code($code);
            )*
            pub const ALL: &[Code] = &[$($name),*];
        };
    }

    define_codes! {
        /// A go/bump step is missing its `deps` input, or the input is not a non-empty list.
        ASSEMBLY_011 = "RPMX-ASSEMBLY-011",
        /// A go/bump dependency is not a map, or lacks a non-empty `module` and `version`.
        ASSEMBLY_012 = "RPMX-ASSEMBLY-012",
        /// A go/bump step lists the same module more than once.
        ASSEMBLY_013 = "RPMX-ASSEMBLY-013",
        /// A go/bump dependency uses a floating version instead of an exact one.
        ASSEMBLY_014 = "RPMX-ASSEMBLY-014",
        /// A go/bump dependency does not explain why it is bumped.
        ASSEMBLY_015 = "RPMX-ASSEMBLY-015",
        /// A go/bump step is not followed by go/vendor, so the SRPM would need the network.
        ASSEMBLY_016 = "RPMX-ASSEMBLY-016",
        /// The go/bump dependency list cannot be serialised to YAML.
        ASSEMBLY_020 = "RPMX-ASSEMBLY-020",
        /// The build or rebuild output directory cannot be created.
        BUILD_001 = "RPMX-BUILD-001",
        /// A `--cell` value does not name a cell in the plan.
        BUILD_002 = "RPMX-BUILD-002",
        /// A per-cell command was run on a multi-cell plan without selecting `--cell`.
        BUILD_003 = "RPMX-BUILD-003",
        /// A built-in pipeline step has no executable command implemented.
        BUILD_004 = "RPMX-BUILD-004",
        /// The build or rebuild work directory cannot be created.
        BUILD_005 = "RPMX-BUILD-005",
        /// The build or rebuild staging and source directories or files cannot be created or written.
        BUILD_006 = "RPMX-BUILD-006",
        /// A component inventory or bundled declaration cannot be read, parsed or written.
        BUNDLED_001 = "RPMX-BUNDLED-001",
        /// Two explicit bundled declarations disagree about a component's version.
        BUNDLED_002 = "RPMX-BUNDLED-002",
        /// A bundled declaration directory is malformed or targets an unknown output.
        BUNDLED_003 = "RPMX-BUNDLED-003",
        /// A bootstrap manifest is malformed, has an unsupported version, or contains an
        /// unsupported field or disagreeing source-date-epoch.
        BOOTSTRAP_001 = "RPMX-BOOTSTRAP-001",
        /// A bootstrap stage name, path, output, or input reference is invalid.
        BOOTSTRAP_002 = "RPMX-BOOTSTRAP-002",
        /// Bootstrap recipe discovery or selection failed (missing package, duplicate name,
        /// multi-cell recipe, or `packages: all` on a seed/cross stage).
        BOOTSTRAP_003 = "RPMX-BOOTSTRAP-003",
        /// A bootstrap dependency cycle has neither an `order` edge nor a `fixed-point`
        /// exemption.
        BOOTSTRAP_004 = "RPMX-BOOTSTRAP-004",
        /// A bootstrap fixed-point rebuild did not converge.
        BOOTSTRAP_005 = "RPMX-BOOTSTRAP-005",
        /// A `--locked` bootstrap stage lock is missing or has drifted.
        BOOTSTRAP_006 = "RPMX-BOOTSTRAP-006",
        /// A bootstrap stage prerequisite (input output, repository, or lock) is missing.
        BOOTSTRAP_007 = "RPMX-BOOTSTRAP-007",
        /// A bootstrap stage output or root already exists and `--force` was not given.
        BOOTSTRAP_008 = "RPMX-BOOTSTRAP-008",
        /// A bootstrap architecture or stage selection is invalid or ambiguous.
        BOOTSTRAP_009 = "RPMX-BOOTSTRAP-009",
        /// Build-time test setup or execution failed (test root, stage, install, argv, or I/O).
        BUILD_007 = "RPMX-BUILD-007",
        /// A staged ELF object does not match the package target architecture.
        BUILD_008 = "RPMX-BUILD-008",
        /// A staged ELF object needs a library the target sysroot lacks.
        BUILD_009 = "RPMX-BUILD-009",
        /// Cache garbage collection was refused without references, or the cache cannot be read.
        CACHE_001 = "RPMX-CACHE-001",
        /// A cache entry could not be removed during garbage collection.
        CACHE_002 = "RPMX-CACHE-002",
        /// The lint command could not serialise its JSON output.
        CLI_001 = "RPMX-CLI-001",
        /// The plan command could not serialise its JSON output.
        CLI_002 = "RPMX-CLI-002",
        /// The srpm command could not serialise its JSON output.
        CLI_003 = "RPMX-CLI-003",
        /// The build command could not serialise its JSON output.
        CLI_004 = "RPMX-CLI-004",
        /// The rebuild command could not serialise its JSON output.
        CLI_005 = "RPMX-CLI-005",
        /// A command could not serialise its machine-readable output (JSON or YAML).
        CLI_006 = "RPMX-CLI-006",
        /// A command-line argument has an invalid value or path.
        CLI_007 = "RPMX-CLI-007",
        /// An RPM or SRPM file cannot be opened as an RPM package.
        EXTRACT_001 = "RPMX-EXTRACT-001",
        /// The RPM payload format cannot be read, or its compression is unsupported.
        EXTRACT_002 = "RPMX-EXTRACT-002",
        /// A gzip- or zstd-compressed RPM payload cannot be decompressed.
        EXTRACT_003 = "RPMX-EXTRACT-003",
        /// The RPM payload is not a valid newc cpio archive, or embedded SRPM metadata is invalid.
        EXTRACT_004 = "RPMX-EXTRACT-004",
        /// An RPM payload entry has an unsafe path (empty, absolute, or containing `..`).
        EXTRACT_005 = "RPMX-EXTRACT-005",
        /// An RPM payload or embedded SRPM metadata file cannot be read or written.
        EXTRACT_006 = "RPMX-EXTRACT-006",
        /// A source cannot use the CAS because it has no valid sha512 digest.
        FETCH_001 = "RPMX-FETCH-001",
        /// A source or locator is incomplete, or no locator could fetch the source.
        FETCH_002 = "RPMX-FETCH-002",
        /// A git, HTTP, or filesystem operation failed while fetching a source.
        FETCH_003 = "RPMX-FETCH-003",
        /// A fetched source's digest or git commit does not match the pinned value.
        FETCH_004 = "RPMX-FETCH-004",
        /// A source cannot be published to or read from the cache, or the cache path is unknown.
        FETCH_005 = "RPMX-FETCH-005",
        /// A source is absent from the cache while `--offline` is set.
        FETCH_006 = "RPMX-FETCH-006",
        /// A source locator kind has no fetcher implementation.
        FETCH_007 = "RPMX-FETCH-007",
        /// A recipe file cannot be read from disk.
        IO_001 = "RPMX-IO-001",
        /// A recipe file or an included fragment cannot be read or resolved.
        INCLUDE_001 = "RPMX-INCLUDE-001",
        /// A top-level section is defined in more than one recipe file.
        INCLUDE_002 = "RPMX-INCLUDE-002",
        /// The merged recipe has no `schema` or no `package` section.
        INCLUDE_003 = "RPMX-INCLUDE-003",
        /// An include path escapes the root recipe's directory.
        INCLUDE_004 = "RPMX-INCLUDE-004",
        /// Recipe includes form a cycle.
        INCLUDE_005 = "RPMX-INCLUDE-005",
        /// An included fragment declares `schema` instead of leaving it to the root.
        INCLUDE_006 = "RPMX-INCLUDE-006",
        /// An init target already exists and --force was not given.
        INIT_001 = "RPMX-INIT-001",
        /// A built package violates package policy.
        POLICY_001 = "RPMX-POLICY-001",
        /// The recipe has one or more lint errors.
        LINT_001 = "RPMX-LINT-001",
        /// A license is not a syntactically valid SPDX expression.
        LINT_010 = "RPMX-LINT-010",
        /// A package release starts at zero.
        LINT_011 = "RPMX-LINT-011",
        /// A summary is empty or unreasonably long.
        LINT_012 = "RPMX-LINT-012",
        /// A recipe omits a deterministic source date.
        LINT_013 = "RPMX-LINT-013",
        /// A dependency or tool is listed twice.
        LINT_014 = "RPMX-LINT-014",
        /// A source, output or test name is declared twice.
        LINT_015 = "RPMX-LINT-015",
        /// A declared variable is never referenced.
        LINT_016 = "RPMX-LINT-016",
        /// A patch file does not exist.
        LINT_017 = "RPMX-LINT-017",
        /// A source locator is not encrypted.
        LINT_018 = "RPMX-LINT-018",
        /// A package declares no maintainers.
        LINT_019 = "RPMX-LINT-019",
        /// A test has neither an argv nor a pipeline.
        LINT_020 = "RPMX-LINT-020",
        /// A build has an empty pipeline.
        LINT_021 = "RPMX-LINT-021",
        /// An output name violates the package naming convention.
        LINT_022 = "RPMX-LINT-022",
        /// A package lock cannot be read, parsed or written.
        LOCK_001 = "RPMX-LOCK-001",
        /// The recipe or plan has drifted from its package lock.
        LOCK_002 = "RPMX-LOCK-002",
        /// A locked build was requested but no package lock exists.
        LOCK_003 = "RPMX-LOCK-003",
        /// The matrix block declares no axes.
        MATRIX_001 = "RPMX-MATRIX-001",
        /// A matrix axis has an empty value list.
        MATRIX_002 = "RPMX-MATRIX-002",
        /// The matrix expands to more cells than `max-cells` allows.
        MATRIX_003 = "RPMX-MATRIX-003",
        /// `matrix.exclude` removes every generated cell.
        MATRIX_004 = "RPMX-MATRIX-004",
        /// Two matrix cells produce the same cell id.
        MATRIX_005 = "RPMX-MATRIX-005",
        /// Two matrix cells produce the same slug.
        MATRIX_006 = "RPMX-MATRIX-006",
        /// A map-valued axis entry has no scalar `value`.
        MATRIX_007 = "RPMX-MATRIX-007",
        /// A map-valued axis entry's `value` is not a scalar.
        MATRIX_008 = "RPMX-MATRIX-008",
        /// An axis value is neither a scalar nor a map with `value`.
        MATRIX_009 = "RPMX-MATRIX-009",
        /// A `matrix.exclude` entry references an axis that does not exist.
        MATRIX_010 = "RPMX-MATRIX-010",
        /// A `matrix.exclude` value for an axis is not a scalar.
        MATRIX_011 = "RPMX-MATRIX-011",
        /// A recipe declares no outputs.
        OUTPUT_001 = "RPMX-OUTPUT-001",
        /// An output name is empty.
        OUTPUT_002 = "RPMX-OUTPUT-002",
        /// An output has no file selectors and is not marked as a metapackage.
        OUTPUT_003 = "RPMX-OUTPUT-003",
        /// An output name expands to an empty string.
        OUTPUT_004 = "RPMX-OUTPUT-004",
        /// Two outputs in one cell have the same name.
        OUTPUT_005 = "RPMX-OUTPUT-005",
        /// A staged file is not claimed by any output selector.
        OUTPUT_010 = "RPMX-OUTPUT-010",
        /// A staged file is claimed by more than one output.
        OUTPUT_011 = "RPMX-OUTPUT-011",
        /// An output matches no staged files.
        OUTPUT_012 = "RPMX-OUTPUT-012",
        /// An output file selector is not a valid glob pattern.
        OUTPUT_013 = "RPMX-OUTPUT-013",
        /// The compiled output selector set cannot be built.
        OUTPUT_014 = "RPMX-OUTPUT-014",
        /// A directory or attribute path is not absolute, normalised, or non-root.
        OUTPUT_015 = "RPMX-OUTPUT-015",
        /// Output file metadata is invalid (mode, capabilities, owner/group, or symlink attributes).
        OUTPUT_016 = "RPMX-OUTPUT-016",
        /// An output declares the same path twice or as conflicting kinds.
        OUTPUT_017 = "RPMX-OUTPUT-017",
        /// An output sets attributes for a path that no packaged file matches.
        OUTPUT_018 = "RPMX-OUTPUT-018",
        /// An output uses an unknown file preset.
        OUTPUT_019 = "RPMX-OUTPUT-019",
        /// Two cells produce the same source package.
        PLAN_001 = "RPMX-PLAN-001",
        /// Two cells produce an output with the same name.
        PLAN_002 = "RPMX-PLAN-002",
        /// `package.name` expands to an empty string.
        PLAN_003 = "RPMX-PLAN-003",
        /// `package.release` expands to an empty string.
        PLAN_004 = "RPMX-PLAN-004",
        /// Output declarations cannot be serialised for template expansion.
        PLAN_005 = "RPMX-PLAN-005",
        /// Expanded output declarations cannot be deserialised.
        PLAN_006 = "RPMX-PLAN-006",
        /// Test declarations cannot be serialised for template expansion.
        PLAN_007 = "RPMX-PLAN-007",
        /// Expanded test declarations cannot be deserialised.
        PLAN_008 = "RPMX-PLAN-008",
        /// Pipeline steps cannot be serialised for template expansion.
        PLAN_009 = "RPMX-PLAN-009",
        /// Expanded pipeline steps cannot be deserialised.
        PLAN_010 = "RPMX-PLAN-010",
        /// A package name, version, or release contains invalid characters.
        PLAN_011 = "RPMX-PLAN-011",
        /// A dist tag contains characters that are not allowed.
        PLAN_012 = "RPMX-PLAN-012",
        /// Repository files cannot be read, created, copied, or written.
        REPO_001 = "RPMX-REPO-001",
        /// The repository input directory contains no RPM files.
        REPO_002 = "RPMX-REPO-002",
        /// An RPM's metadata cannot be read while building repository metadata.
        REPO_003 = "RPMX-REPO-003",
        /// Repository metadata cannot be gzip-compressed.
        REPO_004 = "RPMX-REPO-004",
        /// Running or inspecting the resolver container via podman failed.
        ROOT_001 = "RPMX-ROOT-001",
        /// A builder image is missing, unpullable, undigested, drifted, or the wrong architecture.
        ROOT_002 = "RPMX-ROOT-002",
        /// Reserved. No call site currently emits this code.
        ROOT_003 = "RPMX-ROOT-003",
        /// A root lock, manifest, or resolver state file cannot be read or written.
        ROOT_004 = "RPMX-ROOT-004",
        /// Reserved. No call site currently emits this code.
        ROOT_005 = "RPMX-ROOT-005",
        /// A profile, package manifest, or root lock is malformed, unsupported, or fails validation.
        ROOT_006 = "RPMX-ROOT-006",
        /// dnf resolution or a repository metadata query failed.
        ROOT_007 = "RPMX-ROOT-007",
        /// dnf resolved no packages.
        ROOT_008 = "RPMX-ROOT-008",
        /// Root resolution failed: no packages, unknown architecture, multiarch, or ambiguous provenance.
        ROOT_009 = "RPMX-ROOT-009",
        /// A downloaded package or its RPM header cannot be read.
        ROOT_010 = "RPMX-ROOT-010",
        /// A CAS digest is invalid, or a CAS payload cannot be created, read, or verified.
        ROOT_011 = "RPMX-ROOT-011",
        /// Build-root assembly failed during staging, install, directory setup, or cleanup.
        ROOT_012 = "RPMX-ROOT-012",
        /// The assembled root cannot be queried, or its package set differs from the lock.
        ROOT_013 = "RPMX-ROOT-013",
        /// A root tree cannot be read or walked while computing its digest.
        ROOT_014 = "RPMX-ROOT-014",
        /// A root lock is missing an architecture, builder image, or packages, or has a malformed EVR.
        ROOT_015 = "RPMX-ROOT-015",
        /// Root inputs are missing or inconsistent (`--root`, `--root-lock`, `--root-image`, arch).
        ROOT_016 = "RPMX-ROOT-016",
        /// The assembled root digest does not match the root lock.
        ROOT_017 = "RPMX-ROOT-017",
        /// Required build tools or host/test packages are missing from the selected root.
        ROOT_018 = "RPMX-ROOT-018",
        /// Repository credentials are missing, unreadable, rejected by the
        /// server, or contain an invalid value.
        ROOT_019 = "RPMX-ROOT-019",
        /// A VM build needs a QEMU appliance and the active lock has none.
        ROOT_020 = "RPMX-ROOT-020",
        /// A QEMU appliance lock is malformed, has an unsupported schema, or
        /// its closure digest does not match.
        ROOT_021 = "RPMX-ROOT-021",
        /// A file, symlink, or directory cannot be added to an RPM or SRPM.
        RPM_001 = "RPMX-RPM-001",
        /// The RPM library failed to build an SRPM.
        RPM_002 = "RPMX-RPM-002",
        /// An SRPM cannot be written to disk.
        RPM_003 = "RPMX-RPM-003",
        /// A file or directory has an invalid mode for packaging.
        RPM_004 = "RPMX-RPM-004",
        /// The RPM library failed to build a binary RPM.
        RPM_011 = "RPMX-RPM-011",
        /// Invalid file capabilities, or a rich dependency that is not supported yet.
        RPM_012 = "RPMX-RPM-012",
        /// A dependency string cannot be parsed.
        RPM_013 = "RPMX-RPM-013",
        /// An architecture or runner name is unknown.
        RUNNER_001 = "RPMX-RUNNER-001",
        /// The selected runner cannot execute the target architecture, or qemu/binfmt is unavailable.
        RUNNER_002 = "RPMX-RUNNER-002",
        /// The qemu runner backend failed (ssh, kernel, disk image, copy, or process setup).
        RUNNER_003 = "RPMX-RUNNER-003",
        /// The QEMU appliance is missing, failed to boot, failed to mount the
        /// build root, or failed to enter the chroot.
        RUNNER_004 = "RPMX-RUNNER-004",
        /// A sandbox or process cannot be started.
        SANDBOX_001 = "RPMX-SANDBOX-001",
        /// A sandboxed step exited unsuccessfully.
        SANDBOX_002 = "RPMX-SANDBOX-002",
        /// A step tries to set a reserved sandbox environment variable.
        SANDBOX_003 = "RPMX-SANDBOX-003",
        /// A sandboxed step exceeded its timeout.
        SANDBOX_004 = "RPMX-SANDBOX-004",
        /// An SBOM cannot be read, written, or serialised.
        SBOM_001 = "RPMX-SBOM-001",
        /// The recipe schema version is not supported by this build.
        SCHEMA_001 = "RPMX-SCHEMA-001",
        /// A CPE field is invalid (part, vendor, product, or another component).
        SCHEMA_002 = "RPMX-SCHEMA-002",
        /// A signing key cannot be read, parsed, or resolved to exactly one certificate.
        SIGN_001 = "RPMX-SIGN-001",
        /// Signing data or an RPM signature cannot be produced, read, or written.
        SIGN_002 = "RPMX-SIGN-002",
        /// A signing key cannot be generated or serialised.
        SIGN_003 = "RPMX-SIGN-003",
        /// Signing parameters or timestamps are invalid (key size, user ID, or key creation time).
        SIGN_004 = "RPMX-SIGN-004",
        /// The signing passphrase environment variable is not set.
        SIGN_005 = "RPMX-SIGN-005",
        /// A generated key file cannot be written or have its permissions set.
        SIGN_006 = "RPMX-SIGN-006",
        /// A source uses a digest algorithm other than sha512.
        SOURCE_001 = "RPMX-SOURCE-001",
        /// A source digest or git locator is malformed.
        SOURCE_002 = "RPMX-SOURCE-002",
        /// The source snapshot directory does not exist.
        SOURCE_010 = "RPMX-SOURCE-010",
        /// A source snapshot directory or entry cannot be read.
        SOURCE_011 = "RPMX-SOURCE-011",
        /// A source snapshot contains a symlink, which is not allowed.
        SOURCE_012 = "RPMX-SOURCE-012",
        /// A source snapshot entry is not a regular file.
        SOURCE_013 = "RPMX-SOURCE-013",
        /// A source snapshot path cannot be made relative to the snapshot root.
        SOURCE_014 = "RPMX-SOURCE-014",
        /// A source snapshot file cannot be read.
        SOURCE_015 = "RPMX-SOURCE-015",
        /// A source snapshot path is not valid UTF-8.
        SOURCE_016 = "RPMX-SOURCE-016",
        /// A source snapshot file's metadata cannot be read.
        SOURCE_017 = "RPMX-SOURCE-017",
        /// An SRPM lock or manifest cannot be serialised to JSON.
        SOURCE_018 = "RPMX-SOURCE-018",
        /// A source snapshot path is nested, but SRPM payloads must be flat.
        SOURCE_020 = "RPMX-SOURCE-020",
        /// Source assembly cannot create its workspace, stage, or output directory.
        SOURCE_030 = "RPMX-SOURCE-030",
        /// Patch staging, source materialisation, or snapshot unpacking failed.
        SOURCE_031 = "RPMX-SOURCE-031",
        /// The source snapshot tarball cannot be created or written.
        SOURCE_032 = "RPMX-SOURCE-032",
        /// The recipe context directory is missing, escapes the recipe directory, or is not a directory.
        SOURCE_033 = "RPMX-SOURCE-033",
        /// A context file collides with an upstream source or another context entry.
        SOURCE_034 = "RPMX-SOURCE-034",
        /// The recipe context directory cannot be copied into the workspace.
        SOURCE_035 = "RPMX-SOURCE-035",
        /// The SRPM input file cannot be read.
        SRPM_001 = "RPMX-SRPM-001",
        /// The SRPM output file cannot be created.
        SRPM_002 = "RPMX-SRPM-002",
        /// A local pipeline step has no pipeline directory to load it from.
        SRPM_003 = "RPMX-SRPM-003",
        /// A local step file cannot be read while packaging the SRPM.
        SRPM_004 = "RPMX-SRPM-004",
        /// A local step file is not valid UTF-8.
        SRPM_005 = "RPMX-SRPM-005",
        /// The package lock or an embedded root lock cannot be read, parsed, serialised, or has an unsupported schema.
        SRPM_006 = "RPMX-SRPM-006",
        /// A source file uses a name reserved for SRPM metadata.
        SRPM_007 = "RPMX-SRPM-007",
        /// A step's content changed after the plan was made.
        SRPM_009 = "RPMX-SRPM-009",
        /// A locked built-in step is not embedded in this rpmx build.
        SRPM_010 = "RPMX-SRPM-010",
        /// Rebuild found a step missing from or different from the package lock.
        SRPM_011 = "RPMX-SRPM-011",
        /// A rebuild flag conflicts with the locked dist tag, build release, or package identity.
        SRPM_012 = "RPMX-SRPM-012",
        /// The stage directory does not exist.
        STAGE_001 = "RPMX-STAGE-001",
        /// A stage directory or entry cannot be read.
        STAGE_002 = "RPMX-STAGE-002",
        /// A staged symlink has a non-UTF-8 target.
        STAGE_003 = "RPMX-STAGE-003",
        /// A staged entry is not a regular file.
        STAGE_004 = "RPMX-STAGE-004",
        /// A staged path cannot be made relative to the stage root.
        STAGE_005 = "RPMX-STAGE-005",
        /// A staged file or symlink cannot be read.
        STAGE_006 = "RPMX-STAGE-006",
        /// A staged file's metadata cannot be read.
        STAGE_007 = "RPMX-STAGE-007",
        /// A step has no command, or does not specify exactly one of `uses`, `argv`, or `runs`.
        STEP_001 = "RPMX-STEP-001",
        /// A step's `argv` list is empty.
        STEP_002 = "RPMX-STEP-002",
        /// A step's `runs` script is empty.
        STEP_003 = "RPMX-STEP-003",
        /// A step name is not a valid namespaced name.
        STEP_010 = "RPMX-STEP-010",
        /// A step file cannot be read, or the named step cannot be found.
        STEP_011 = "RPMX-STEP-011",
        /// A step file is not UTF-8 or does not parse as a valid step.
        STEP_012 = "RPMX-STEP-012",
        /// A step file's declared name does not match the requested name.
        STEP_013 = "RPMX-STEP-013",
        /// A step is given an input it does not declare.
        STEP_014 = "RPMX-STEP-014",
        /// A required step input is missing, or an input reference is unknown.
        STEP_015 = "RPMX-STEP-015",
        /// A local step shadows a built-in step, or an input value has the wrong type.
        STEP_016 = "RPMX-STEP-016",
        /// A step has an empty `argv`, or specifies both or neither of `argv` and `runs`.
        STEP_017 = "RPMX-STEP-017",
        /// A step requests an unknown template renderer.
        STEP_018 = "RPMX-STEP-018",
        /// A step definition hardcodes a guest path instead of using a variable or placeholder.
        STEP_019 = "RPMX-STEP-019",
        /// A step timeout is not a positive whole-number duration with s, m or h suffix.
        STEP_020 = "RPMX-STEP-020",
        /// Variable definitions form a cycle.
        TEMPLATE_001 = "RPMX-TEMPLATE-001",
        /// A template references an unknown variable.
        TEMPLATE_002 = "RPMX-TEMPLATE-002",
        /// A variable value is not a scalar.
        TEMPLATE_003 = "RPMX-TEMPLATE-003",
        /// A template expression is not terminated.
        TEMPLATE_004 = "RPMX-TEMPLATE-004",
        /// A template expansion references an unknown variable.
        TEMPLATE_005 = "RPMX-TEMPLATE-005",
        /// A template substitution resolves to a non-scalar value.
        TEMPLATE_006 = "RPMX-TEMPLATE-006",
        /// A template expansion left an unresolved expression in the output.
        TEMPLATE_007 = "RPMX-TEMPLATE-007",
        /// A test name expands to an empty string.
        TEST_001 = "RPMX-TEST-001",
        /// Two tests in one cell have the same name.
        TEST_002 = "RPMX-TEST-002",
        /// A test step failed or timed out.
        TEST_003 = "RPMX-TEST-003",
        /// A test declaration uses an unsupported stage or has no command.
        TEST_004 = "RPMX-TEST-004",
        /// Installed-stage tests need a test root, which needs a root lock.
        TEST_005 = "RPMX-TEST-005",
        /// A recipe declares no tests.
        TEST_006 = "RPMX-TEST-006",
        /// A source-assembly tool uses a digest algorithm other than sha512.
        TOOL_001 = "RPMX-TOOL-001",
        /// A source-assembly tool has a malformed sha512 digest.
        TOOL_002 = "RPMX-TOOL-002",
        /// A source-assembly tool is declared more than once.
        TOOL_003 = "RPMX-TOOL-003",
        /// A source-assembly tool has an empty name or version.
        TOOL_004 = "RPMX-TOOL-004",
        /// A source-assembly tool declares no locators.
        TOOL_005 = "RPMX-TOOL-005",
        /// A source-assembly tool locator has an empty kind.
        TOOL_006 = "RPMX-TOOL-006",
        /// A recipe is not valid YAML.
        YAML_001 = "RPMX-YAML-001",
        /// A YAML document cannot be parsed.
        YAML_002 = "RPMX-YAML-002",
        /// A YAML document uses aliases or anchors, which are not allowed.
        YAML_003 = "RPMX-YAML-003",
        /// A YAML document uses a custom tag, which is not allowed.
        YAML_004 = "RPMX-YAML-004",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: Code,
    pub message: String,
    pub path: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    /// The command already printed a document describing this failure.
    #[doc(hidden)]
    pub reported: bool,
}

impl Error {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
            line: None,
            column: None,
            reported: false,
        }
    }

    /// Mark this error as already rendered by the command.
    pub fn reported(mut self) -> Self {
        self.reported = true;
        self
    }

    pub fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_location(mut self, line: Option<usize>, column: Option<usize>) -> Self {
        self.line = line;
        self.column = column;
        self
    }

    /// Reclassify an error that escaped from a later stage.
    ///
    /// A sandbox step failure during a test is a test failure, not a build
    /// failure, even though both surface as `RPMX-SANDBOX-002`.
    pub fn with_code(mut self, code: Code) -> Self {
        self.code = code;
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(path) = &self.path {
            write!(f, " at {path}")?;
        }
        if let Some(line) = self.line {
            write!(f, " (line {line}")?;
            if let Some(column) = self.column {
                write!(f, ", column {column}")?;
            }
            write!(f, ")")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::codes;

    #[test]
    fn every_code_is_unique_and_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for code in codes::ALL {
            let text = code.as_str();
            assert!(seen.insert(text), "duplicate diagnostic code {text}");
            let rest = text
                .strip_prefix("RPMX-")
                .unwrap_or_else(|| panic!("code {text} lacks RPMX- prefix"));
            let (domain, number) = rest
                .rsplit_once('-')
                .unwrap_or_else(|| panic!("code {text} lacks a number"));
            assert!(
                !domain.is_empty() && domain.chars().all(|ch| ch.is_ascii_uppercase()),
                "code {text} has an invalid domain"
            );
            assert!(
                number.len() == 3 && number.chars().all(|ch| ch.is_ascii_digit()),
                "code {text} has an invalid number"
            );
        }
    }
}
