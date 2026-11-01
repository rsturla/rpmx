use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::{Error, Recipe};

/// Resolve the deterministic build timestamp from the CLI override and the
/// recipe. A missing value stays `0`, which is the historical unsigned default.
pub fn source_date_epoch(recipe: &Recipe, explicit: Option<u32>) -> u32 {
    explicit.or(recipe.package.source_date_epoch).unwrap_or(0)
}

/// Choose the signature creation time. Signed builds must be reproducible, so
/// an explicit timestamp or a non-zero source date epoch is required.
pub fn signature_timestamp(
    explicit: Option<u32>,
    source_date_epoch: u32,
    signing: Option<&rpmx_rpm::sign::SigningKey>,
) -> Result<Option<u32>, Error> {
    let Some(key) = signing else {
        return Ok(None);
    };
    let created_at = key.created_at();
    let timestamp = match explicit {
        Some(seconds) => seconds,
        None if source_date_epoch == 0 => {
            return Err(Error::new(
                codes::SIGN_004,
                "a signed build needs a deterministic timestamp; \
                 set package.source-date-epoch, pass --source-date-epoch, or pass --signing-timestamp",
            ));
        }
        // A key cannot vouch for a time before it existed. Clamp to its
        // creation time so the result stays deterministic and verifiable.
        None => source_date_epoch.max(created_at),
    };
    if timestamp < created_at {
        return Err(Error::new(
            codes::SIGN_004,
            format!(
                "signature timestamp {timestamp} is before signing key {} was created at {created_at}",
                key.fingerprint()
            ),
        ));
    }
    Ok(Some(timestamp))
}

/// Load the optional signing key selected on the command line.
pub fn load_signing_key(
    path: Option<&Path>,
    passphrase_env: Option<&str>,
) -> Result<Option<rpmx_rpm::sign::SigningKey>, Error> {
    let Some(path) = path else {
        return Ok(None);
    };
    let passphrase = signing_passphrase(passphrase_env)?;
    Ok(Some(rpmx_rpm::sign::load_key(path, passphrase.as_deref())?))
}

/// Resolve the passphrase for a signing key from an environment variable.
/// The passphrase is never accepted as a command-line argument.
pub fn signing_passphrase(variable: Option<&str>) -> Result<Option<String>, Error> {
    let Some(variable) = variable else {
        return Ok(None);
    };
    std::env::var(variable).map(Some).map_err(|_| {
        Error::new(
            codes::SIGN_005,
            format!("signing passphrase environment variable {variable} is not set"),
        )
    })
}
