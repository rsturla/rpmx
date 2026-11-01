use rpmx_core::error::codes;
use rpmx_core::Error;
use rpmx_rpm::repo::{generate, RepoOptions};

use crate::commands::{load_signing_key, RepoArgs};

/// Write repodata for an output directory in place.
pub fn index(
    output: &std::path::Path,
    signing_key: Option<&rpmx_rpm::sign::SigningKey>,
    timestamp: u32,
) -> Result<rpmx_rpm::repo::RepoReport, Error> {
    generate(RepoOptions {
        rpms: output,
        output,
        signing_key,
        timestamp,
    })
}

pub fn run(args: RepoArgs) -> Result<(), Error> {
    let key = load_signing_key(
        args.signing.signing_key.as_deref(),
        args.signing.signing_passphrase_env.as_deref(),
    )?;
    let timestamp = args.signing.signing_timestamp.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as u32)
            .unwrap_or(0)
    });
    let report = generate(RepoOptions {
        rpms: &args.rpms,
        output: &args.output,
        signing_key: key.as_ref(),
        timestamp,
    })?;

    if rpmx_core::output::json() {
        let value = crate::commands::report::success(
            "repo",
            serde_json::json!({
                "packages": report.packages,
                "repo": args.output.display().to_string(),
                "repomd": report.repomd.display().to_string(),
                "signature": report.signature.as_ref().map(|path| path.display().to_string()),
                "signing_key": key.as_ref().map(|key| key.fingerprint()),
            }),
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_003, error.to_string()))?
        );
    } else {
        println!("{} packages", report.packages);
        println!("{}", report.repomd.display());
        if let Some(path) = report.signature {
            println!("{}", path.display());
        }
    }
    Ok(())
}
