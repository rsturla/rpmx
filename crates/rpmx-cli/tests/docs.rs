//! Keep user-facing docs honest about the command surface.
//!
//! Every `rpmx ...` example in README.md and docs/usage/ is checked against
//! the binary's own help: the command path must exist and every long flag must
//! be accepted. This is the cheap half of the contract; behavioural claims are
//! covered by the command tests.

use std::path::Path;
use std::process::Command;

fn help(path: &[String]) -> String {
    let mut args = path.to_vec();
    args.push("--help".to_string());
    let output = Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(&args)
        .output()
        .expect("run rpmx");
    assert!(
        output.status.success(),
        "rpmx {} --help failed: {}",
        path.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn subcommands(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_commands = false;
    for line in text.lines() {
        if line.trim() == "Commands:" {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(name) = line.split_whitespace().next() {
            if name != "help" {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Walk the non-flag tokens as far down the command tree as they match.
fn command_path(tokens: &[String]) -> Vec<String> {
    let mut path = Vec::new();
    let mut text = help(&path);
    let mut first = true;
    for token in tokens {
        let token = token.trim_matches(|ch| matches!(ch, '`' | '"' | '\''));
        if token.starts_with('-') || token.is_empty() {
            continue;
        }
        if token.starts_with('<') || token.starts_with('[') || token.contains('|') {
            break;
        }
        if subcommands(&text).iter().any(|name| name == token) {
            path.push(token.to_string());
            text = help(&path);
        } else if first {
            panic!(
                "`{token}` is not an rpmx subcommand (in `rpmx {}`)",
                tokens.join(" ")
            );
        }
        first = false;
    }
    path
}

fn documented_commands() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    let mut files = vec![root.join("README.md")];
    if let Ok(entries) = std::fs::read_dir(root.join("docs/usage")) {
        files.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "md")),
        );
    }

    let mut commands = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read doc");
        commands.extend(extract_commands(&text));
    }
    commands
}

fn extract_commands(text: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut in_console = false;
    let mut continued: Option<String> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_console = !in_console;
            continued = None;
            continue;
        }
        if in_console {
            let trimmed = line.trim();
            let line = match continued.take() {
                Some(mut previous) => {
                    previous.push(' ');
                    previous.push_str(trimmed);
                    previous
                }
                None => trimmed.to_string(),
            };
            if line.ends_with('\\') {
                continued = Some(line.trim_end_matches('\\').trim_end().to_string());
                continue;
            }
            if let Some(command) = strip_prompt(&line) {
                commands.push(command);
            }
            continue;
        }
        let negative = line.to_ascii_lowercase();
        if negative.contains("not implemented") || negative.contains("not yet") {
            continue;
        }
        for span in line.split('`').skip(1).step_by(2) {
            if let Some(command) = strip_prompt(span.trim()) {
                commands.push(command);
            }
        }
    }
    commands
}

fn strip_prompt(line: &str) -> Option<String> {
    let line = line.strip_prefix("$ ").unwrap_or(line);
    let line = line.strip_prefix("./target/release/").unwrap_or(line);
    line.strip_prefix("rpmx ").map(|rest| rest.to_string())
}

#[test]
fn documented_commands_and_flags_exist() {
    let mut checked = 0;
    for command in documented_commands() {
        let tokens: Vec<String> = command
            .split_whitespace()
            .take_while(|token| !token.starts_with('#'))
            .map(str::to_string)
            .collect();
        let path = command_path(&tokens);
        let text = help(&path);
        for token in &tokens {
            let Some(flag) = token
                .split('=')
                .next()
                .filter(|flag| flag.starts_with("--"))
            else {
                continue;
            };
            assert!(
                text.contains(flag),
                "documented command `rpmx {command}` uses {flag}, which `rpmx {} --help` does not accept",
                path.join(" "),

            );
        }
        checked += 1;
    }
    assert!(checked >= 20, "only checked {checked} documented commands");
}

/// Guard the walker itself: a path that exists must be found, otherwise the
/// test could pass by checking nothing.
#[test]
fn the_walker_finds_nested_commands() {
    let tokens: Vec<String> = ["root", "resolve", "profile.yaml"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(command_path(&tokens), ["root", "resolve"]);
}
