//! `Cargo.lock` cataloger.
//!
//! The lockfile is a small TOML document. Parsing the package blocks directly
//! avoids a TOML dependency and keeps discovery deterministic. Packages with a
//! `source` entry are registry or git dependencies; packages without one are
//! workspace or path packages and are not bundled.

use std::collections::BTreeSet;

use rpmx_base::error::{codes, Error, Result};

use crate::discover::{read_text, Cataloger, DiscoveryContext};
use crate::model::{Component, Ecosystem, Scope, Source};

pub(crate) struct CargoLock;

impl Cataloger for CargoLock {
    fn name(&self) -> &'static str {
        "cargo/lock"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        let mut components = Vec::new();
        for file in context.files() {
            if file.file_name().and_then(|name| name.to_str()) != Some("Cargo.lock") {
                continue;
            }
            let vendored = file
                .parent()
                .is_some_and(|parent| parent.join("vendor").is_dir());
            let text = read_text(file)?;
            components.extend(parse(
                &text,
                &context.relative(file),
                if vendored {
                    Scope::Vendored
                } else {
                    Scope::Declared
                },
                context.auto_outputs(),
            )?);
        }
        Ok(components)
    }
}

fn parse(
    text: &str,
    evidence: &str,
    scope: Scope,
    outputs: &BTreeSet<String>,
) -> Result<Vec<Component>> {
    let mut components = Vec::new();
    let mut block = Block::default();
    let mut in_package = false;

    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            flush(&mut block, &mut components, evidence, scope, outputs)?;
            in_package = true;
            continue;
        }
        if !in_package {
            continue;
        }
        if line.starts_with('[') {
            flush(&mut block, &mut components, evidence, scope, outputs)?;
            in_package = false;
            continue;
        }
        if let Some(value) = value(line, "name") {
            block.name = Some(value);
        } else if let Some(value) = value(line, "version") {
            block.version = Some(value);
        } else if let Some(value) = value(line, "source") {
            block.source = Some(value);
        }
    }
    flush(&mut block, &mut components, evidence, scope, outputs)?;
    Ok(components)
}

#[derive(Default)]
struct Block {
    name: Option<String>,
    version: Option<String>,
    source: Option<String>,
}

fn flush(
    block: &mut Block,
    components: &mut Vec<Component>,
    evidence: &str,
    scope: Scope,
    outputs: &BTreeSet<String>,
) -> Result<()> {
    let Some(name) = block.name.take() else {
        block.version = None;
        block.source = None;
        return Ok(());
    };
    let version = block.version.take();
    let source = block.source.take();
    let Some(version) = version else {
        return Err(Error::new(
            codes::BUNDLED_001,
            format!("package '{name}' in {evidence} has no version"),
        ));
    };
    let Some(source) = source else {
        return Ok(());
    };
    if !source.starts_with("registry+") && !source.starts_with("git+") {
        return Ok(());
    }
    let version = if source.starts_with("git+") {
        let reference = source.rsplit_once('#').map(|(_, reference)| reference);
        match reference {
            Some(reference)
                if reference.len() >= 8 && reference.chars().all(|ch| ch.is_ascii_hexdigit()) =>
            {
                format!(
                    "{version}+git{}",
                    reference.chars().take(8).collect::<String>()
                )
            }
            _ => version,
        }
    } else {
        version
    };
    components.push(
        Component::new(
            Ecosystem::Cargo,
            name,
            Some(version),
            Source::Auto,
            scope,
            outputs.iter().cloned(),
        )
        .with_evidence("cargo/lock", evidence),
    );
    Ok(())
}

fn value(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim();
    let rest = rest.strip_prefix('"')?.strip_suffix('"')?;
    Some(rest.to_string())
}
