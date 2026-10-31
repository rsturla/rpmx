use std::process::Command;

fn help(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args(args)
        .output()
        .expect("run rpmx")
}

#[test]
fn subcommand_help_parses_without_argument_collisions() {
    for args in [
        vec!["--help"],
        vec!["lint", "--help"],
        vec!["plan", "--help"],
        vec!["fetch", "--help"],
        vec!["source", "--help"],
        vec!["srpm", "--help"],
        vec!["rebuild", "--help"],
        vec!["build", "--help"],
        vec!["repo", "--help"],
        vec!["keygen", "--help"],
        vec!["sign", "--help"],
        vec!["root", "resolve", "--help"],
        vec!["root", "assemble", "--help"],
        vec!["root", "digest", "--help"],
        vec!["root", "export", "--help"],
        vec!["root", "import", "--help"],
        vec!["cache", "gc", "--help"],
    ] {
        let output = help(&args);
        assert!(
            output.status.success(),
            "rpmx {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
