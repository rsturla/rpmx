use std::process::Command;

#[test]
fn replay_and_resolution_are_mutually_exclusive() {
    let output = Command::new(env!("CARGO_BIN_EXE_rpmx"))
        .args([
            "test",
            "--install-dir",
            "rpms",
            "--test-profile",
            "runtime.yaml",
            "--replay",
            "saved",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn production_commands_do_not_accept_package_test_configuration() {
    for arguments in [
        vec!["build", "--test-profile", "runtime.yaml"],
        vec![
            "rebuild",
            "source.src.rpm",
            "--test-profile",
            "runtime.yaml",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rpmx"))
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("unexpected argument '--test-profile'"));
    }
}
