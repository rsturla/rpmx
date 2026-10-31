use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;
use rpmx_rpm::repo::{generate, RepoOptions};

use crate::commands::load_signing_key;

pub struct RepoArgs<'a> {
    pub rpms: &'a Path,
    pub output: &'a Path,
    pub signing_key: Option<&'a Path>,
    pub signing_passphrase_env: Option<&'a str>,
    pub signing_timestamp: Option<u32>,
    pub json: bool,
}

pub fn run(args: RepoArgs<'_>) -> Result<(), Error> {
    let key = load_signing_key(args.signing_key, args.signing_passphrase_env)?;
    let timestamp = args.signing_timestamp.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as u32)
            .unwrap_or(0)
    });
    let report = generate(RepoOptions {
        rpms: args.rpms,
        output: args.output,
        signing_key: key.as_ref(),
        timestamp,
    })?;

    if args.json {
        let value = serde_json::json!({
            "ok": true,
            "packages": report.packages,
            "repo": args.output.display().to_string(),
            "repomd": report.repomd.display().to_string(),
            "signature": report.signature.as_ref().map(|path| path.display().to_string()),
            "signing_key": key.as_ref().map(|key| key.fingerprint()),
        });
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
