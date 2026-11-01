use std::collections::BTreeSet;
use std::process::Command;

fn help(args: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .expect("run rpmx")
}

/// Discover subcommand names from clap's `Commands:` block.
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

/// Walk the whole command tree instead of a hand-maintained list. Clap builds
/// every subcommand while parsing its `--help`, so an argument collision in a
/// newly added command fails here even when nobody remembered to add it.
#[test]
fn every_subcommand_help_parses() {
    let mut visited = BTreeSet::new();
    let mut pending = vec![Vec::<String>::new()];
    while let Some(path) = pending.pop() {
        let mut args = path.clone();
        args.push("--help".to_string());
        let output = help(&args);
        assert!(
            output.status.success(),
            "rpmx {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("--json"),
            "rpmx {} --help does not expose the global --json flag",
            args.join(" ")
        );
        if !path.is_empty() {
            visited.insert(path.join(" "));
        }
        for name in subcommands(&String::from_utf8_lossy(&output.stdout)) {
            let mut child = path.clone();
            child.push(name);
            pending.push(child);
        }
    }

    for command in [
        "explain", "shell", "steps", "doctor", "bump", "init", "lock", "test",
    ] {
        assert!(
            visited.contains(command),
            "help walk never reached `{command}`; visited {visited:?}"
        );
    }
    assert!(visited.len() >= 20, "only visited {visited:?}");
}
