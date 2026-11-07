use std::path::PathBuf;

use rpmx_core::Error;

use crate::commands::report::{self, ArtifactGroup};

/// Report rebuilt binaries unless the command runs as a quiet step.
pub(super) fn print_artifacts(
    quiet: bool,
    json: bool,
    written: &[PathBuf],
    srpms: &[PathBuf],
    evidence: &[PathBuf],
    repository: &[PathBuf],
    signing_key: Option<&str>,
) -> Result<(), Error> {
    if quiet {
        return Ok(());
    }
    report::print(
        json,
        "rebuild",
        &[
            ArtifactGroup {
                kind: "rpm",
                paths: written,
            },
            ArtifactGroup {
                kind: "srpm",
                paths: srpms,
            },
            ArtifactGroup {
                kind: "evidence",
                paths: evidence,
            },
            ArtifactGroup {
                kind: "repository",
                paths: repository,
            },
        ],
        signing_key,
    )
}
