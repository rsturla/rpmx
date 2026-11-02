//! Go `vendor/modules.txt` cataloger.

use std::collections::BTreeSet;

use rpmx_base::error::{codes, Error, Result};

use crate::discover::{read_text, Cataloger, DiscoveryContext};
use crate::model::{Component, Ecosystem, Scope, Source};

pub(crate) struct GoModules;

impl Cataloger for GoModules {
    fn name(&self) -> &'static str {
        "go/modules-txt"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        let mut components = Vec::new();
        for file in context.files() {
            if file.file_name().and_then(|name| name.to_str()) != Some("modules.txt") {
                continue;
            }
            if !file
                .parent()
                .and_then(std::path::Path::file_name)
                .is_some_and(|parent| parent == "vendor")
            {
                continue;
            }
            let text = read_text(file)?;
            let evidence = context.relative(file);
            components.extend(parse(&text, &evidence, context.auto_outputs())?);
        }
        Ok(components)
    }
}

fn parse(text: &str, evidence: &str, outputs: &BTreeSet<String>) -> Result<Vec<Component>> {
    let mut components = Vec::new();
    for line in text.lines() {
        let Some(line) = line.strip_prefix("# ") else {
            continue;
        };
        let Some((name, version)) = parse_line(line) else {
            continue;
        };
        if name.is_empty() {
            return Err(Error::new(
                codes::BUNDLED_001,
                format!("invalid Go module line in {evidence}: {line}"),
            ));
        }
        components.push(
            Component::new(
                Ecosystem::Golang,
                name,
                version,
                Source::Auto,
                Scope::Vendored,
                outputs.iter().cloned(),
            )
            .with_evidence("go/modules-txt", evidence),
        );
    }
    Ok(components)
}

fn parse_line(line: &str) -> Option<(String, Option<String>)> {
    let line = line.trim();
    if let Some((before, after)) = line.split_once(" => ") {
        let after = after.trim();
        if after.starts_with("./") || after.starts_with("../") || after.is_empty() {
            let mut parts = before.split_whitespace();
            let name = parts.next()?.to_string();
            return Some((name, parts.next().map(str::to_string)));
        }
        let mut parts = after.split_whitespace();
        let name = parts.next()?.to_string();
        return Some((name, parts.next().map(str::to_string)));
    }
    let mut parts = line.split_whitespace();
    let name = parts.next()?.to_string();
    Some((name, parts.next().map(str::to_string)))
}
