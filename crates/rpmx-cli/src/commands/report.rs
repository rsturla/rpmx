use std::path::PathBuf;

use rpmx_core::error::codes;
use rpmx_core::Error;

pub struct ArtifactGroup<'a> {
    pub key: &'a str,
    pub paths: &'a [PathBuf],
}

/// Print command artifacts as plain paths or one JSON object.
pub fn print(
    json: bool,
    groups: &[ArtifactGroup<'_>],
    signing_key: Option<&str>,
    include_signing_key: bool,
) -> Result<(), Error> {
    if json {
        let mut value = serde_json::Map::new();
        value.insert("ok".to_string(), serde_json::Value::Bool(true));
        for group in groups {
            value.insert(
                group.key.to_string(),
                serde_json::Value::Array(
                    group
                        .paths
                        .iter()
                        .map(|path| serde_json::Value::String(path.display().to_string()))
                        .collect(),
                ),
            );
        }
        if include_signing_key {
            value.insert(
                "signing_key".to_string(),
                signing_key.map_or(serde_json::Value::Null, |key| {
                    serde_json::Value::String(key.to_string())
                }),
            );
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_004, error.to_string()))?
        );
    } else {
        for group in groups {
            for path in group.paths {
                println!("{}", path.display());
            }
        }
    }
    Ok(())
}
