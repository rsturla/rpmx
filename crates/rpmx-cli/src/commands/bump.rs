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
    if parsed.package.version != args.version {
        return Err(Error::new(
            codes::INIT_001,
            format!(
                "the rewritten recipe parses version '{}', not '{}'; refusing to write",
                parsed.package.version, args.version
            ),
        ));
    }

    if !args.no_fetch {
        let base = args.file.parent().unwrap_or_else(|| Path::new("."));
        let cache = default_cache()?;
        let mut lines: Vec<String> = updated.lines().map(str::to_string).collect();
        let digest_lines: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.trim_start().starts_with("digest:"))
            .map(|(index, _)| index)
            .collect();
        if digest_lines.len() != parsed.sources.len() {
            return Err(Error::new(
                codes::INIT_001,
                "cannot match every source to its digest line; update the recipe by hand",
            ));
        }
        for (source, index) in parsed.sources.iter().zip(digest_lines) {
            let digest = rpmx_core::fetch::fetch_digest(source, base, &cache)?;
            lines[index] = replace_scalar(&lines[index], "digest", &digest);
        }
        updated = lines.join("\n") + if updated.ends_with('\n') { "\n" } else { "" };
    }

    if args.dry_run {
        let diff = render_diff(&text, &updated);
        if args.json {
            eprintln!("{diff}");
        } else {
            print!("{diff}");
        }
    } else {
        write_atomically(&args.file, &updated)?;
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

fn write_atomically(path: &Path, text: &str) -> Result<(), Error> {
    let temporary = path.with_extension("yaml.tmp");
    std::fs::write(&temporary, text).map_err(|error| {
        Error::new(
            codes::IO_001,
            format!("cannot write {}: {error}", temporary.display()),
        )
    })?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        Error::new(
            codes::IO_001,
            format!("cannot replace {}: {error}", path.display()),
        )
    })
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
            *line = replace_scalar(line, "version", new);
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

/// Replace a scalar value, preserving indentation, quote style and a trailing
/// comment.
fn replace_scalar(line: &str, key: &str, value: &str) -> String {
    let indent = &line[..line.len() - line.trim_start().len()];
    let rest = line
        .trim_start()
        .strip_prefix(key)
        .unwrap_or("")
        .trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
    let (quoted, comment) = match rest.find('#') {
        Some(position) => (rest[..position].trim_end(), &rest[position..]),
        None => (rest, ""),
    };
    let rendered = if quoted.starts_with('"') {
        format!("\"{value}\"")
    } else if quoted.starts_with('\'') {
        format!("'{value}'")
    } else {
        value.to_string()
    };
    let suffix = if comment.is_empty() {
        String::new()
    } else {
        format!(" {comment}")
    };
    format!("{indent}{key}: {rendered}{suffix}")
}

fn replace_url_versions(text: &str, old: &str, new: &str) -> String {
    let mut in_sources = false;
    text.lines()
        .map(|line| {
            if !line.starts_with(' ') && line.trim_end().ends_with(':') {
                in_sources = line.trim_end() == "sources:";
            }
            if in_sources && line.trim_start().starts_with("url:") {
                line.replace(old, new)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if text.ends_with('\n') { "\n" } else { "" }
}

fn render_diff(old: &str, new: &str) -> String {
    let mut diff = String::new();
    let old: Vec<_> = old.lines().collect();
    let new: Vec<_> = new.lines().collect();
    for (left, right) in old.iter().zip(new.iter()) {
        if left != right {
            diff.push_str(&format!("-{left}\n+{right}\n"));
        }
    }
    for line in old.iter().skip(new.len()) {
        diff.push_str(&format!("-{line}\n"));
    }
    for line in new.iter().skip(old.len()) {
        diff.push_str(&format!("+{line}\n"));
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::{render_diff, replace_scalar, replace_url_versions, replace_version};

    const RECIPE: &str = r#"package:
  name: probe
  version: "1.0" # keep this pin
  release: 1
sources:
  - name: probe.tar.gz
    digest: "sha512:aaaa"
    locators:
      - kind: https
        url: https://example.invalid/probe-1.0.tar.gz
build:
  pipeline:
    - runs: |
        echo "url: https://service.example/api/1.0"
"#;

    #[test]
    fn the_package_version_and_urls_move_together() {
        let text = replace_version(RECIPE, "1.0", "1.10").expect("version");
        let text = replace_url_versions(&text, "1.0", "1.10");
        assert!(text.contains("version: \"1.10\" # keep this pin"), "{text}");
        assert!(text.contains("probe-1.10.tar.gz"), "{text}");
        // A URL in a heredoc is not a source locator.
        assert!(text.contains("api/1.0"), "{text}");
    }

    #[test]
    fn scalar_replacement_preserves_comments_and_quotes() {
        assert_eq!(
            replace_scalar("  version: '1.0' # pin", "version", "2.0"),
            "  version: '2.0' # pin"
        );
        assert_eq!(
            replace_scalar("    digest: sha512:aa", "digest", "sha512:bb"),
            "    digest: sha512:bb"
        );
    }

    #[test]
    fn the_diff_only_shows_changed_lines() {
        let diff = render_diff("a\nb\n", "a\nc\n");
        assert_eq!(diff, "-b\n+c\n");
    }
}
