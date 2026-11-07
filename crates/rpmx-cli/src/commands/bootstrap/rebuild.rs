use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::BootstrapRebuildArgs;

/// Rebuild one package against an explicitly selected stage root lock and
/// re-emit its SRPM. Choosing this command is the opt-in to replacing the
/// embedded lock; plain `rpmx rebuild` stays strict.
pub fn run(args: BootstrapRebuildArgs) -> Result<(), Error> {
    let input = args.rebuild.file.clone();
    prepare_srpm_output(&args.srpm_out, &input)?;
    let output = crate::commands::rebuild::run_with(args.rebuild, true, Some(&args.srpm_out))?;
    let _ = output.rpms;
    for path in &output.srpms {
        rpmx_core::output::phase("bootstrap", format!("replacement SRPM {}", path.display()));
    }
    Ok(())
}

/// Refuse an output directory that would overwrite the input SRPM. The
/// replacement is a new artifact; the input must stay byte-identical.
fn prepare_srpm_output(output: &Path, input: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(output).map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot create {}: {error}", output.display()),
        )
    })?;
    let output = std::fs::canonicalize(output).map_err(|error| {
        Error::new(
            codes::SRPM_002,
            format!("cannot resolve {}: {error}", output.display()),
        )
    })?;
    if let Some(parent) = input.parent() {
        if let Ok(parent) = std::fs::canonicalize(parent) {
            if parent == output {
                return Err(Error::new(
                    codes::SRPM_002,
                    "--srpm-out must differ from the input SRPM's directory; \
                     the input SRPM is never modified",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::prepare_srpm_output;

    #[test]
    fn the_input_srpm_directory_is_refused() {
        let directory = tempfile::tempdir().expect("tempdir");
        let input = directory.path().join("probe.src.rpm");
        std::fs::write(&input, b"srpm").expect("input");
        let error = prepare_srpm_output(directory.path(), &input).expect_err("same dir");
        assert_eq!(error.code, "RPMX-SRPM-002");
        assert!(
            error.message.contains("never modified"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_different_output_directory_is_accepted() {
        let directory = tempfile::tempdir().expect("tempdir");
        let input = directory.path().join("probe.src.rpm");
        std::fs::write(&input, b"srpm").expect("input");
        let output = directory.path().join("out/src");
        prepare_srpm_output(&output, &input).expect("output");
        assert!(output.is_dir());
    }
}
