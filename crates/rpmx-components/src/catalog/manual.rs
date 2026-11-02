//! Explicit C/C++ (and other generic) bundled declarations.

use std::path::Path;

use rpmx_base::error::{codes, Error, Result};

use crate::discover::{Cataloger, DiscoveryContext};
use crate::model::{Component, Ecosystem, Scope, Source};
use crate::BUNDLED_DIR;

const MAX_LIST_BYTES: u64 = 1024 * 1024;

pub(crate) struct ManualBuild;

impl Cataloger for ManualBuild {
    fn name(&self) -> &'static str {
        "manual/build"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        discover(context)
    }
}

fn discover(context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
    let rpmx = context.root().join(".rpmx");
    if let Ok(metadata) = std::fs::symlink_metadata(&rpmx) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::new(
                codes::BUNDLED_003,
                format!("{} must be a real directory", rpmx.display()),
            ));
        }
    }
    let base = context.root().join(BUNDLED_DIR);
    if !base.exists() {
        return Ok(Vec::new());
    }
    let metadata = std::fs::symlink_metadata(&base).map_err(|error| {
        Error::new(
            codes::BUNDLED_003,
            format!("cannot stat {}: {error}", base.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::new(
            codes::BUNDLED_003,
            format!("{} must be a real directory", base.display()),
        ));
    }

    let mut components = Vec::new();
    for entry in rpmx_base::io::sorted_entries(&base, codes::BUNDLED_003)? {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            return Err(Error::new(
                codes::BUNDLED_003,
                format!(
                    "{} is not an output directory; only <output>/*.list is allowed",
                    path.display()
                ),
            ));
        }
        let output = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if !context.outputs().contains(&output) {
            return Err(Error::new(
                codes::BUNDLED_003,
                format!(
                    "bundled declaration targets unknown output '{output}' in {}",
                    path.display()
                ),
            ));
        }
        for file in rpmx_base::io::sorted_entries(&path, codes::BUNDLED_003)? {
            let file_path = file.path();
            if !file.file_type().is_ok_and(|kind| kind.is_file()) {
                return Err(Error::new(
                    codes::BUNDLED_003,
                    format!("{} is not a regular declaration file", file_path.display()),
                ));
            }
            if file_path.extension().and_then(|ext| ext.to_str()) != Some("list") {
                return Err(Error::new(
                    codes::BUNDLED_003,
                    format!(
                        "{} is not a .list declaration; only *.list is read",
                        file_path.display()
                    ),
                ));
            }
            let relative = context.relative(&file_path);
            components.extend(parse_file(&file_path, &relative, &output)?);
        }
    }
    Ok(components)
}

fn parse_file(path: &Path, evidence: &str, output: &str) -> Result<Vec<Component>> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        Error::new(
            codes::BUNDLED_003,
            format!("cannot stat {}: {error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::new(
            codes::BUNDLED_003,
            format!("{} is not a regular file", path.display()),
        ));
    }
    if metadata.len() > MAX_LIST_BYTES {
        return Err(Error::new(
            codes::BUNDLED_003,
            format!("{} exceeds the {MAX_LIST_BYTES}-byte limit", path.display()),
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::BUNDLED_003,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let mut components = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, version) = match line.split_once('=') {
            Some((name, version)) => (name.trim(), Some(version.trim().to_string())),
            None => (line, None),
        };
        if !valid_name(name) {
            return Err(Error::new(
                codes::BUNDLED_003,
                format!(
                    "{}:{}: invalid bundled name '{name}'",
                    path.display(),
                    index + 1
                ),
            ));
        }
        if let Some(version) = &version {
            if !valid_version(version) {
                return Err(Error::new(
                    codes::BUNDLED_003,
                    format!(
                        "{}:{}: invalid bundled version '{version}'",
                        path.display(),
                        index + 1
                    ),
                ));
            }
        }
        components.push(
            Component::new(
                Ecosystem::Generic,
                name,
                version,
                Source::Build,
                Scope::Vendored,
                [output.to_string()],
            )
            .with_evidence("manual/build", evidence),
        );
    }
    Ok(components)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '_' | '-'))
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && !version.chars().any(|ch| {
            ch.is_whitespace() || ch.is_control() || matches!(ch, '=' | '(' | ')' | '/' | ':' | '-')
        })
}
