//! Ruby `Gemfile.lock` and installed gemspec cataloger.

use rpmx_base::error::Result;

use crate::discover::{read_text, Cataloger, DiscoveryContext};
use crate::model::{Component, Ecosystem, Scope, Source};

pub(crate) struct RubyGems;

impl Cataloger for RubyGems {
    fn name(&self) -> &'static str {
        "ruby/gems"
    }

    fn detect(&self, context: &DiscoveryContext<'_>) -> Result<Vec<Component>> {
        let mut components = Vec::new();
        for file in context.files() {
            let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let evidence = context.relative(file);
            if name == "Gemfile.lock" {
                let scope = if file
                    .parent()
                    .is_some_and(|root| root.join("vendor/bundle").is_dir())
                {
                    Scope::Vendored
                } else {
                    Scope::Declared
                };
                let text = read_text(file)?;
                for (name, version) in lock_specs(&text) {
                    if scope == Scope::Vendored
                        && file.parent().is_some_and(|root| {
                            !gem_installed(&root.join("vendor/bundle"), &name, &version)
                        })
                    {
                        continue;
                    }
                    components.push(component(
                        name,
                        Some(version),
                        scope,
                        "ruby/gemfile-lock",
                        &evidence,
                        context,
                    ));
                }
            } else if name.ends_with(".gemspec") && is_vendored_path(file) {
                let text = read_text(file)?;
                if let Some((name, version)) = gemspec(&text) {
                    components.push(component(
                        name,
                        version,
                        Scope::Installed,
                        "ruby/gemspec",
                        &evidence,
                        context,
                    ));
                }
            }
        }
        Ok(components)
    }
}

fn component(
    name: String,
    version: Option<String>,
    scope: Scope,
    cataloger: &str,
    evidence: &str,
    context: &DiscoveryContext<'_>,
) -> Component {
    Component::new(
        Ecosystem::Ruby,
        name,
        version,
        Source::Auto,
        scope,
        context.auto_outputs().iter().cloned(),
    )
    .with_evidence(cataloger, evidence)
}

fn lock_specs(text: &str) -> Vec<(String, String)> {
    let mut specs = Vec::new();
    let mut in_specs = false;
    let mut skip = false;
    let mut section = "";
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if indent == 0 && matches!(trimmed, "PATH" | "GIT" | "GEM") {
            section = trimmed;
            skip = false;
            in_specs = false;
            continue;
        }
        if section == "PATH" && trimmed == "remote: ." {
            skip = true;
        }
        if trimmed == "specs:" {
            in_specs = !skip;
            continue;
        }
        if !in_specs {
            continue;
        }
        if trimmed.is_empty() {
            in_specs = false;
            continue;
        }
        // Resolved specs are at one level below `specs:`; dependency
        // constraints are nested one level deeper.
        if indent != 4 {
            continue;
        }
        let Some((name, rest)) = trimmed.split_once(" (") else {
            continue;
        };
        let Some(version) = rest.strip_suffix(')') else {
            continue;
        };
        if name.is_empty()
            || version.is_empty()
            || name.contains(' ')
            || version
                .chars()
                .any(|ch| ch.is_whitespace() || matches!(ch, '<' | '>' | '=' | '~'))
        {
            continue;
        }
        specs.push((name.to_string(), version.to_string()));
    }
    specs
}

fn gemspec(text: &str) -> Option<(String, Option<String>)> {
    let mut name = None;
    let mut version = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let key = key
            .strip_prefix("s.")
            .or_else(|| key.strip_prefix("spec."))
            .unwrap_or(key);
        let value = value.trim();
        let value = value.strip_suffix(".freeze").unwrap_or(value).trim();
        let quoted = (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''));
        if !quoted || value.len() < 2 {
            continue;
        }
        let value = &value[1..value.len() - 1];
        if key == "name" {
            name = Some(value.to_string());
        } else if key == "version" {
            version = Some(value.to_string());
        }
    }
    name.map(|name| (name, version))
}

fn gem_installed(root: &std::path::Path, name: &str, version: &str) -> bool {
    let expected_dir = format!("{name}-{version}");
    let expected_spec = format!("{name}-{version}.gemspec");
    fn walk(directory: &std::path::Path, dir: &str, spec: &str, depth: u8) -> bool {
        if depth > 6 {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            return false;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if file_name == dir || file_name == spec {
                return true;
            }
            if path.is_dir() && walk(&path, dir, spec, depth + 1) {
                return true;
            }
        }
        false
    }
    walk(root, &expected_dir, &expected_spec, 0)
}

fn is_vendored_path(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| matches!(part, "vendor" | "gems" | "specifications"))
    })
}

#[cfg(test)]
mod tests {
    use super::{gemspec, lock_specs};

    #[test]
    fn gemfile_lock_specs_are_read() {
        assert_eq!(
            lock_specs(
                "GEM\n  remote: https://rubygems.org/\n  specs:\n    rake (13.3.1)\n      dep (>= 1.0)\n    foo (1.0.0)\n\nDEPENDENCIES\n  rake\n"
            ),
            vec![
                ("rake".to_string(), "13.3.1".to_string()),
                ("foo".to_string(), "1.0.0".to_string())
            ]
        );
    }

    #[test]
    fn path_section_is_not_a_bundled_dependency() {
        let specs = lock_specs(
            "PATH\n  remote: .\n  specs:\n    mygem (1.0.0)\n\nGEM\n  specs:\n    rake (13.3.1)\n",
        );
        assert!(specs.contains(&("rake".to_string(), "13.3.1".to_string())));
        assert!(!specs.iter().any(|(name, _)| name == "mygem"));
    }

    #[test]
    fn gemspec_name_and_version_are_read() {
        assert_eq!(
            gemspec(
                "s.name = \"rake\".freeze\ns.version = \"13.3.1\"\ns.required_ruby_version = Gem::Requirement.new(\">= 2.3.0\")\n"
            ),
            Some(("rake".to_string(), Some("13.3.1".to_string())))
        );
        assert_eq!(
            gemspec("s.name = \"sample\"\ns.version = version\n"),
            Some(("sample".to_string(), None))
        );
    }
}
