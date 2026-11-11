//! Compile test commands into the existing runner interface.

use crate::model::Test;
use rpmx_base::error::{codes, Error, Result};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn command(test: &Test) -> Result<Option<Vec<String>>> {
    let invalid = |message: &str| Error::new(codes::TEST_004, message);
    if test.argv.is_some() && test.pipeline.is_some() {
        return Err(invalid("choose one test form: argv, script or pipeline"));
    }
    if test.argv.as_ref().is_some_and(|argv| {
        argv.first()
            .is_some_and(|name| matches!(name.rsplit('/').next(), Some("sh" | "bash")))
            && argv
                .iter()
                .skip(1)
                .any(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
    }) {
        return Err(invalid(
            "shell commands in test argv are not supported; use script",
        ));
    }
    if let Some(script) = &test.script {
        if test.argv.is_some() || test.pipeline.is_some() || test.expect.is_some() {
            return Err(invalid(
                "test script cannot be combined with argv, pipeline or expect",
            ));
        }
        if script.trim().is_empty() {
            return Err(invalid("test script must not be empty"));
        }
        return Ok(Some(bash(script.clone(), true)));
    }
    let Some(expect) = &test.expect else {
        return Ok(test.argv.clone());
    };
    let argv = test
        .argv
        .as_ref()
        .filter(|argv| !argv.is_empty())
        .ok_or_else(|| invalid("test expect needs a nonempty argv"))?;
    if argv.iter().any(|arg| arg.contains('\0'))
        || expect
            .stdout
            .iter()
            .chain(&expect.stderr)
            .any(|value| value.contains('\0'))
    {
        return Err(invalid("test assertions do not support NUL bytes"));
    }
    // The sentinel preserves trailing newlines in command substitutions.
    let mut script = format!(
        "test_tmp=$(mktemp -d)\ntrap 'rm -rf -- \"$test_tmp\"' EXIT\nset +e\n{} >\"$test_tmp/stdout\" 2>\"$test_tmp/stderr\"\ntest_status=$?\nset -e\ncat \"$test_tmp/stdout\"\ncat \"$test_tmp/stderr\" >&2\nif [[ $test_status != {} ]]; then\n  printf 'expected exit {}, got %s\\n' \"$test_status\" >&2\n  exit 1\nfi\n",
        argv.iter().map(|arg| quote(arg)).collect::<Vec<_>>().join(" "),
        expect.exit_code, expect.exit_code,
    );
    for (stream, expected) in [("stdout", &expect.stdout), ("stderr", &expect.stderr)] {
        if let Some(expected) = expected {
            script.push_str(&format!(
                "test_actual=$(cat \"$test_tmp/{stream}\"; printf x)\ntest_expected={}\nif [[ $test_actual != \"${{test_expected}}x\" ]]; then\n  printf '%s\\n' {} >&2\n  printf 'expected: %q\\nactual:   %q\\n' \"$test_expected\" \"${{test_actual%x}}\" >&2\n  exit 1\nfi\n",
                quote(expected), quote(&format!("{stream} differs")),
            ));
        }
    }
    // Expected nonzero statuses and exact stream checks already have their own
    // diagnostics. Do not add an ERR trap around the command being asserted.
    Ok(Some(bash(script, false)))
}

fn bash(script: String, diagnostics: bool) -> Vec<String> {
    // Capture the failure before running the handler changes $? or PIPESTATUS.
    // Printing leaves errexit in control, including its if/&&/||/! exceptions.
    const PREFIX: &str = r#"rpmx_test_report_error() {
  local failure_status=$1 failure_line=$2 failure_command=$3 failure_pipeline=$4 failure_source=$5
  if [[ -z $failure_source || $failure_source == "$0" ]]; then
    failure_line=$((failure_line - RPMX_PREFIX_LINES))
    failure_source=script
  fi
  printf 'rpmx: test %s line %s failed (exit %s; pipeline statuses: %s); last command: %s\n' "$failure_source" "$failure_line" "$failure_status" "$failure_pipeline" "$failure_command" >&2 || :
}
trap 'rpmx_test_report_error "$?" "$LINENO" "$BASH_COMMAND" "${PIPESTATUS[*]}" "${BASH_SOURCE[0]:-}"' ERR
"#;
    let script = if diagnostics {
        let prefix = PREFIX.replace("RPMX_PREFIX_LINES", &PREFIX.lines().count().to_string());
        format!("{prefix}{script}")
    } else {
        script
    };
    [
        "bash",
        "--noprofile",
        "--norc",
        if diagnostics { "-Eeuo" } else { "-euo" },
        "pipefail",
        "-c",
    ]
    .into_iter()
    .map(String::from)
    .chain([script])
    .collect()
}

pub(super) fn strict_step(step: &mut super::PlannedStep) {
    if matches!(step.command.as_deref(), Some("sh" | "bash"))
        && step.args.first().map(String::as_str) == Some("-c")
        && step.args.len() == 2
    {
        let mut argv = bash(step.args[1].clone(), true);
        step.command = Some(argv.remove(0));
        step.args = argv;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execute(yaml: &str) -> std::process::Output {
        let test: Test = serde_yaml_ng::from_str(yaml).expect("test");
        let argv = command(&test).expect("command").expect("argv");
        std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
            .expect("execute")
    }

    fn execute_script(script: &str) -> std::process::Output {
        let mut test: Test =
            serde_yaml_ng::from_str("name: probe\nmode: installed\nscript: placeholder\n")
                .expect("test");
        test.script = Some(script.into());
        let argv = command(&test).expect("command").expect("argv");
        std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
            .expect("execute")
    }

    #[test]
    fn script_failures_report_original_line_command_and_status() {
        let output = execute_script("printf 'before\\n'\ntest 755 = 777\nprintf 'after\\n'\n");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(output.stdout, b"before\n");
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(stderr.contains("line 2 failed (exit 1"), "{stderr}");
        assert!(stderr.contains("last command: test 755 = 777"), "{stderr}");

        let output = execute_script("bash -c 'exit 7'\nprintf 'unreachable\\n'\n");
        assert_eq!(output.status.code(), Some(7));
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(stderr.contains("line 1 failed (exit 7"), "{stderr}");
        assert!(
            stderr.contains("last command: bash -c 'exit 7'"),
            "{stderr}"
        );
    }

    #[test]
    fn failures_inside_nested_functions_keep_the_failing_body_line() {
        let output =
            execute_script("inner() {\n  test present = missing\n}\nouter() { inner; }\nouter\n");
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(stderr.contains("line 2 failed (exit 1"), "{stderr}");
        assert!(
            stderr.contains("last command: test present = missing"),
            "{stderr}"
        );
        assert_eq!(stderr.matches("rpmx: test script").count(), 1, "{stderr}");
    }

    #[test]
    fn sourced_fixtures_keep_their_own_filename_and_line_numbers() {
        let fixture = tempfile::NamedTempFile::new().expect("fixture");
        std::fs::write(
            fixture.path(),
            "fixture_assertion() {\n  test present = missing\n}\nfixture_assertion\n",
        )
        .expect("write fixture");
        let output = execute_script(&format!(
            "source {}\n",
            quote(fixture.path().to_str().unwrap())
        ));
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(
            stderr.contains(&format!(
                "{} line 2 failed (exit 1",
                fixture.path().display()
            )),
            "{stderr}"
        );
        assert!(
            stderr.contains("last command: test present = missing"),
            "{stderr}"
        );
        assert_eq!(stderr.matches("rpmx: test ").count(), 1, "{stderr}");
    }

    #[test]
    fn pipeline_diagnostics_identify_the_last_command_and_all_statuses() {
        let output = execute_script("printf 'before\\n'\nbash -c 'exit 7' | true\n");
        assert_eq!(output.status.code(), Some(7));
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(stderr.contains("line 2 failed (exit 7"), "{stderr}");
        assert!(stderr.contains("pipeline statuses: 7 0"), "{stderr}");
        assert!(stderr.contains("last command: true"), "{stderr}");
    }

    #[test]
    fn expected_script_failures_do_not_emit_diagnostics() {
        let output = execute_script(
            "if false; then exit 11; fi\nfalse || true\n! false\nif false | true; then exit 12; fi\nif value=$(false); then exit 14; fi\nhandled() { false; printf 'handled\\n'; }\nhandled || exit 13\nprintf 'done\\n'\n",
        );
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"handled\ndone\n");
        assert!(output.stderr.is_empty(), "{output:?}");
    }

    #[test]
    fn command_substitution_reports_inner_failure_and_assignment_without_recursion() {
        let output = execute_script("printf 'before\\n'\nvalue=$(bash -c 'exit 7')\n");
        assert_eq!(output.status.code(), Some(7));
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(stderr.contains("line 2 failed (exit 7"), "{stderr}");
        assert!(
            stderr.contains("last command: bash -c 'exit 7'"),
            "{stderr}"
        );
        assert!(
            stderr.contains("last command: value=$(bash -c 'exit 7')"),
            "{stderr}"
        );
        assert_eq!(stderr.matches("rpmx: test script").count(), 2, "{stderr}");
    }

    #[test]
    fn temporarily_disabling_errexit_still_preserves_the_command_status() {
        let output =
            execute_script("set +e\nbash -c 'exit 7'\nstatus=$?\nset -e\ntest \"$status\" = 7\n");
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(stderr.contains("line 2 failed (exit 7"), "{stderr}");
        assert_eq!(stderr.matches("rpmx: test script").count(), 1, "{stderr}");
    }

    #[test]
    fn planned_test_pipeline_steps_use_diagnostics_without_changing_build_steps() {
        let recipe = crate::parse_recipe(
            r#"
schema: 2
package:
  name: probe
  version: "1"
  release: 1
  summary: probe
  license: MIT
build:
  pipeline:
    - runs: "false"
outputs:
  - name: probe
    metapackage: true
tests:
  - name: pipeline
    mode: installed
    pipeline:
      - runs: |
          printf 'before\n'
          bash -c 'exit 7' | true
"#,
        )
        .expect("recipe");
        let plan = super::super::plan(&recipe, "x86_64").expect("plan");
        let cell = &plan.cells[0];
        assert_eq!(cell.pipeline[0].command.as_deref(), Some("sh"));
        assert_eq!(cell.pipeline[0].args, ["-c", "false"]);
        let step = &cell.tests[0].pipeline[0];
        let output = std::process::Command::new(step.command.as_ref().expect("command"))
            .args(&step.args)
            .output()
            .expect("execute planned test");
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"before\n");
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(stderr.contains("line 2 failed (exit 7"), "{stderr}");
        assert!(stderr.contains("pipeline statuses: 7 0"), "{stderr}");
    }

    #[test]
    fn scripts_fail_on_early_errors_pipelines_and_unset_variables() {
        for script in [
            "false; true",
            "false | true",
            "echo $RPMX_UNSET_TEST_VARIABLE",
        ] {
            let output = execute(&format!(
                "name: probe\nmode: installed\nscript: '{script}'\n"
            ));
            assert!(!output.status.success(), "{script}");
        }
    }

    #[test]
    fn assertions_preserve_newlines_and_shell_characters() {
        let output = execute(
            r#"
name: probe
mode: installed
argv: [printf, '%s\n', "a'b $HOME `false`"]
expect:
  stdout: "a'b $HOME `false`\n"
  stderr: ''
"#,
        );
        assert!(output.status.success(), "{:?}", output);
    }

    #[test]
    fn assertions_check_nonzero_exit_and_both_streams() {
        let yaml = "name: probe\nmode: installed\nargv: [printf, out]\nexpect:\n  exit-code: 0\n  stdout: out\n  stderr: ''\n";
        assert!(execute(yaml).status.success());
        for changed in [
            yaml.replace("exit-code: 0", "exit-code: 7"),
            yaml.replace("stdout: out", "stdout: wrong"),
            yaml.replace("stderr: ''", "stderr: wrong"),
        ] {
            assert!(!execute(&changed).status.success());
        }
        assert!(execute(
            "name: probe\nmode: installed\nargv: ['false']\nexpect:\n  exit-code: 1\n  stderr: ''\n"
        )
        .status
        .success());
        let output = execute("name: probe\nmode: installed\nargv: ['false']\n");
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty(), "{output:?}");
    }

    #[test]
    fn a_missing_final_newline_fails() {
        let output = execute(
            "name: probe\nmode: installed\nargv: [printf, hello]\nexpect:\n  stdout: \"hello\\n\"\n",
        );
        assert!(!output.status.success());
    }

    #[test]
    fn conflicting_forms_are_rejected() {
        for fields in ["script: true\nargv: [true]", "expect: {}", "script: ''"] {
            let test: Test =
                serde_yaml_ng::from_str(&format!("name: probe\nmode: installed\n{fields}"))
                    .expect("test");
            assert!(command(&test).is_err());
        }
    }
}
