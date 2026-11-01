//! Update a recipe to a new upstream version, including source digests.

use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::{fetch::default_cache, BumpArgs};

pub fn run(args: BumpArgs) -> Result<(), Error> {
    let text = std::fs::read_to_string(&args.file).map_err(|error| {
        Error::new(
            codes::IO_001,
            format!("cannot read {}: {error}", args.file.display()),
        )
    })?;
    let recipe = rpmx_core::parse_recipe(&text)?;
    let old_version = recipe.package.version.clone();
    if old_version == args.version {
        return Err(Error::new(
            codes::INIT_001,
            format!(
                "{} is already at version {}",
                args.file.display(),
                args.version
            ),
        ));
    }
    let mut updated = replace_version(&text, &old_version, &args.version)?;
    updated = replace_url_versions(&updated, &old_version, &args.version);
    let parsed = rpmx_core::parse_recipe(&updated)?;
    if !args.no_fetch {
        let base = args.file.parent().unwrap_or_else(|| Path::new("."));
        let cache = default_cache()?;
        for source in &parsed.sources {
            let digest = rpmx_core::fetch::fetch_digest(source, base, &cache)?;
            updated = updated.replace(&source.digest, &digest);
        }
    }

    if args.dry_run {
        print_diff(&text, &updated);
    } else {
        std::fs::write(&args.file, &updated).map_err(|error| {
            Error::new(
                codes::IO_001,
                format!("cannot write {}: {error}", args.file.display()),
            )
        })?;
    }

    if args.json {
        let value = crate::commands::report::success(
            "bump",
            serde_json::json!({
                "file": args.file.display().to_string(),
                "from": old_version,
                "to": args.version,
                "dry_run": args.dry_run,
            }),
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_006, error.to_string()))?
        );
    } else if !args.dry_run {
        println!("{} -> {}", old_version, args.version);
    }
    Ok(())
}

fn replace_version(text: &str, old: &str, new: &str) -> Result<String, Error> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut in_package = false;
    let mut replaced = false;
    for line in &mut lines {
        let trimmed = line.trim_start();
        if !line.starts_with(' ') && trimmed.ends_with(':') {
            in_package = trimmed == "package:";
            continue;
        }
        if in_package && trimmed.starts_with("version:") {
            let indent = &line[..line.len() - trimmed.len()];
            let value = trimmed.trim_start_matches("version:").trim();
            let quote = value.starts_with('"') && value.ends_with('"');
            *line = if quote {
                format!("{indent}version: \"{new}\"")
            } else {
                format!("{indent}version: {new}")
            };
            replaced = true;
            break;
        }
    }
    if !replaced {
        return Err(Error::new(
            codes::YAML_001,
            format!("cannot find package.version = {old}"),
        ));
    }
    Ok(lines.join("\n") + if text.ends_with('\n') { "\n" } else { "" })
}

fn replace_url_versions(text: &str, old: &str, new: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim_start().starts_with("url:") {
                line.replace(old, new)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if text.ends_with('\n') { "\n" } else { "" }
}

fn print_diff(old: &str, new: &str) {
    let old: Vec<_> = old.lines().collect();
    let new: Vec<_> = new.lines().collect();
    for (left, right) in old.iter().zip(new.iter()) {
        if left != right {
            println!("-{left}");
            println!("+{right}");
        }
    }
    for line in old.iter().skip(new.len()) {
        println!("-{line}");
    }
    for line in new.iter().skip(old.len()) {
        println!("+{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::{replace_url_versions, replace_version};

    const RECIPE: &str = r#"package:
  name: probe
  version: "1.0"
  release: 1
sources:
  - name: probe.tar.gz
    digest: "sha512:aaaa"
    locators:
      - kind: https
        url: https://example.invalid/probe-1.0.tar.gz
"#;

    #[test]
    fn the_package_version_and_urls_move_together() {
        let text = replace_version(RECIPE, "1.0", "1.1").expect("version");
        let text = replace_url_versions(&text, "1.0", "1.1");
        assert!(text.contains("version: \"1.1\""), "{text}");
        assert!(text.contains("probe-1.1.tar.gz"), "{text}");
    }
}
