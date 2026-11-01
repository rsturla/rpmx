use std::path::PathBuf;

use rpmx_core::error::codes;
use rpmx_core::Error;

pub struct ArtifactGroup<'a> {
    /// Artifact kind, such as `rpm`, `srpm`, `sbom`, or `test-root-lock`.
    pub kind: &'a str,
    pub paths: &'a [PathBuf],
}

/// Wrap command-specific fields in the success envelope every JSON command
/// shares.
pub fn success(command: &'static str, value: serde_json::Value) -> serde_json::Value {
    let mut document = match value {
        serde_json::Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("value".to_string(), other);
            map
        }
    };
    document.insert("schema".to_string(), serde_json::json!(1));
    document.insert("ok".to_string(), serde_json::json!(true));
    document.insert("command".to_string(), serde_json::json!(command));
    serde_json::Value::Object(document)
}

/// Build the JSON document for a successful command.
///
/// The shape is the CLI contract: one document, one schema version, and one
/// entry per file with a `kind` the caller can dispatch on.
pub fn document(
    command: &'static str,
    groups: &[ArtifactGroup<'_>],
    signing_key: Option<&str>,
) -> serde_json::Value {
    let artifacts: Vec<_> = groups
        .iter()
        .flat_map(|group| {
            group.paths.iter().map(|path| {
                serde_json::json!({
                    "kind": group.kind,
                    "path": path.display().to_string(),
                })
            })
        })
        .collect();
    let mut value = serde_json::json!({
        "schema": 1,
        "ok": true,
        "command": command,
        "artifacts": artifacts,
    });
    if let (Some(key), Some(object)) = (signing_key, value.as_object_mut()) {
        object.insert(
            "signing_key".to_string(),
            serde_json::Value::String(key.to_string()),
        );
    }
    value
}

/// Print command artifacts as plain paths or one JSON document.
pub fn print(
    json: bool,
    command: &'static str,
    groups: &[ArtifactGroup<'_>],
    signing_key: Option<&str>,
) -> Result<(), Error> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&document(command, groups, signing_key))
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

#[derive(Debug, Clone)]
pub struct TestOutcome {
    pub name: String,
    pub stage: String,
    pub error: Option<String>,
    pub duration: std::time::Duration,
}

/// Aggregate one command's test results into JUnit XML.
pub fn write_junit(path: &std::path::Path, outcomes: &[TestOutcome]) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut tests = existing.matches("<testcase ").count();
    let mut failures = existing.matches("<failure ").count();
    let mut body = existing;
    if let Some(position) = body.rfind("</testsuite>") {
        body.truncate(position);
    } else {
        body.clear();
    }
    if let Some(start) = body.find("<testsuite") {
        if let Some(end) = body[start..].find('>') {
            body.drain(..start + end + 1);
        }
    }
    let mut cases = String::new();
    for outcome in outcomes {
        tests += 1;
        if outcome.error.is_some() {
            failures += 1;
        }
        cases.push_str(&format!(
            "  <testcase name=\"{}\" classname=\"{}\" time=\"{:.3}\">",
            xml_escape(&outcome.name),
            xml_escape(&outcome.stage),
            outcome.duration.as_secs_f64()
        ));
        if let Some(error) = &outcome.error {
            cases.push_str(&format!(
                "<failure message=\"{}\">{}</failure>",
                xml_escape(error),
                xml_escape(error)
            ));
        }
        cases.push_str("</testcase>\n");
    }
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"rpmx\" tests=\"{tests}\" failures=\"{failures}\">\n{}{cases}</testsuite>\n",
        body.trim_start_matches('\n')
    );
    std::fs::write(path, xml)
}

pub fn test_document(outcomes: &[TestOutcome]) -> serde_json::Value {
    serde_json::json!({
        "schema": 1,
        "ok": outcomes.iter().all(|outcome| outcome.error.is_none()),
        "command": "test",
        "tests": outcomes.iter().map(|outcome| serde_json::json!({
            "name": outcome.name,
            "stage": outcome.stage,
            "ok": outcome.error.is_none(),
            "duration_ms": outcome.duration.as_millis() as u64,
            "message": outcome.error,
        })).collect::<Vec<_>>(),
    })
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{document, ArtifactGroup};

    #[test]
    fn json_documents_carry_a_schema_and_typed_artifacts() {
        let rpms = vec![
            std::path::PathBuf::from("out/a.rpm"),
            std::path::PathBuf::from("out/b.rpm"),
        ];
        let srpms = vec![std::path::PathBuf::from("out/a.src.rpm")];
        let value = document(
            "build",
            &[
                ArtifactGroup {
                    kind: "srpm",
                    paths: &srpms,
                },
                ArtifactGroup {
                    kind: "rpm",
                    paths: &rpms,
                },
                ArtifactGroup {
                    kind: "evidence",
                    paths: &[],
                },
            ],
            Some("sha256:key"),
        );
        assert_eq!(value["schema"], 1);
        assert_eq!(value["ok"], true);
        assert_eq!(value["command"], "build");
        assert_eq!(
            value["artifacts"],
            serde_json::json!([
                {"kind": "srpm", "path": "out/a.src.rpm"},
                {"kind": "rpm", "path": "out/a.rpm"},
                {"kind": "rpm", "path": "out/b.rpm"},
            ])
        );
        assert_eq!(value["signing_key"], "sha256:key");
    }
}
