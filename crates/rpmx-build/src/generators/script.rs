use crate::package_spec::BinaryPackageFile;

use super::Findings;

/// `script.req`: an executable script requires the interpreter on its `#!` line.
///
/// An absolute interpreter is required directly. For
/// `#!/usr/bin/env /absolute/interpreter`, both paths are required; a bare
/// `#!/usr/bin/env name` only requires `/usr/bin/env`. rpmx does not rewrite
/// shebangs, so it cannot resolve a bare name to a path.
pub fn inspect(file: &BinaryPackageFile, findings: &mut Findings) {
    if file.mode & 0o111 == 0 || !file.data.starts_with(b"#!") {
        return;
    }
    let end = file
        .data
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(file.data.len());
    let Ok(line) = std::str::from_utf8(&file.data[2..end]) else {
        return;
    };
    let mut words = line.split_whitespace();
    let Some(interpreter) = words.next() else {
        return;
    };
    if interpreter.starts_with('/') && !findings.is_local_path(interpreter) {
        findings.require(interpreter);
    }
    if interpreter.ends_with("/env") {
        if let Some(argument) = words.next() {
            if argument.starts_with('/') && !findings.is_local_path(argument) {
                findings.require(argument);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::test_file;

    fn requires(data: &str) -> Vec<String> {
        let mut findings = Findings::default();
        inspect(&test_file("/usr/bin/tool", data.as_bytes()), &mut findings);
        findings.into_dependencies().1
    }

    #[test]
    fn a_shell_script_requires_its_interpreter() {
        assert_eq!(requires("#!/bin/sh\necho hi\n"), vec!["/bin/sh"]);
    }

    #[test]
    fn env_with_an_absolute_interpreter_requires_both() {
        assert_eq!(
            requires("#!/usr/bin/env /usr/bin/python3\n"),
            vec!["/usr/bin/env", "/usr/bin/python3"]
        );
    }

    #[test]
    fn env_with_a_bare_name_only_requires_env() {
        assert_eq!(requires("#!/usr/bin/env python3\n"), vec!["/usr/bin/env"]);
    }

    #[test]
    fn a_non_executable_file_is_ignored() {
        let mut findings = Findings::default();
        let mut file = test_file("/usr/share/doc/script", b"#!/bin/sh\n");
        file.mode = 0o644;
        inspect(&file, &mut findings);
        assert!(findings.into_dependencies().1.is_empty());
    }

    #[test]
    fn a_local_interpreter_is_not_a_require() {
        let mut findings = Findings::default();
        findings.provide_path("/usr/bin/python3");
        inspect(
            &test_file("/usr/bin/tool", b"#!/usr/bin/python3\n"),
            &mut findings,
        );
        assert!(findings.into_dependencies().1.is_empty());
    }
}
