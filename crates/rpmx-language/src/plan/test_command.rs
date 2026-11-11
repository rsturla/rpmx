//! Compile test commands into the existing runner interface.

use crate::model::Test;
use rpmx_base::error::{codes, Error, Result};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn command(test: &Test) -> Result<Option<Vec<String>>> {
    let invalid = |message: &str| Error::new(codes::TEST_004, message);
    if let Some(script) = &test.script {
        if test.argv.is_some() || test.pipeline.is_some() || test.expect.is_some() {
            return Err(invalid(
                "test script cannot be combined with argv, pipeline or expect",
            ));
        }
        if script.trim().is_empty() {
            return Err(invalid("test script must not be empty"));
        }
        return Ok(Some(bash(script.clone())));
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
    Ok(Some(bash(script)))
}

fn bash(script: String) -> Vec<String> {
    ["bash", "--noprofile", "--norc", "-euo", "pipefail", "-c"]
        .into_iter()
        .map(String::from)
        .chain([script])
        .collect()
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

    #[test]
    fn scripts_fail_on_early_errors_pipelines_and_unset_variables() {
        for script in [
            "false; true",
            "false | true",
            "echo $RPMX_UNSET_TEST_VARIABLE",
        ] {
            let output = execute(&format!("name: probe\nstage: build\nscript: '{script}'\n"));
            assert!(!output.status.success(), "{script}");
        }
    }

    #[test]
    fn assertions_preserve_newlines_and_shell_characters() {
        let output = execute(
            r#"
name: probe
stage: build
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
        let yaml = "name: probe\nstage: build\nargv: [sh, -c, 'printf out; printf err >&2; exit 7']\nexpect:\n  exit-code: 7\n  stdout: out\n  stderr: err\n";
        assert!(execute(yaml).status.success());
        for changed in [
            yaml.replace("exit-code: 7", "exit-code: 0"),
            yaml.replace("stdout: out", "stdout: wrong"),
            yaml.replace("stderr: err", "stderr: wrong"),
        ] {
            assert!(!execute(&changed).status.success());
        }
    }

    #[test]
    fn a_missing_final_newline_fails() {
        let output = execute(
            "name: probe\nstage: build\nargv: [printf, hello]\nexpect:\n  stdout: \"hello\\n\"\n",
        );
        assert!(!output.status.success());
    }

    #[test]
    fn conflicting_forms_are_rejected() {
        for fields in ["script: true\nargv: [true]", "expect: {}", "script: ''"] {
            let test: Test =
                serde_yaml_ng::from_str(&format!("name: probe\nstage: build\n{fields}"))
                    .expect("test");
            assert!(command(&test).is_err());
        }
    }
}
