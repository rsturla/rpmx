//! `debuginfo.attr`: the build-id of a `-debuginfo` file.

use crate::package_spec::BinaryPackageFile;

use super::Findings;

pub fn inspect(file: &BinaryPackageFile, findings: &mut Findings) {
    let Some(dependency) = build_id(&file.path) else {
        return;
    };
    findings.provide(dependency);
}

fn build_id(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/usr/lib/debug/.build-id/")?;
    let (directory, file) = rest.split_once('/')?;
    let file = file.strip_suffix(".debug")?;
    if directory.len() != 2
        || file.len() < 2
        || !directory
            .chars()
            .all(|character| character.is_ascii_hexdigit())
        || !file.chars().all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    Some(format!("debuginfo(build-id) = {directory}{file}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::test_file;

    #[test]
    fn a_build_id_path_provides_the_full_id() {
        let mut findings = Findings::default();
        inspect(
            &test_file(
                "/usr/lib/debug/.build-id/ab/cdef0123456789.debug",
                b"\x7fELF",
            ),
            &mut findings,
        );
        assert_eq!(
            findings.into_dependencies().0,
            vec!["debuginfo(build-id) = abcdef0123456789"]
        );
    }

    #[test]
    fn an_ordinary_debug_file_is_ignored() {
        let mut findings = Findings::default();
        inspect(
            &test_file("/usr/lib/debug/usr/bin/demo.debug", b""),
            &mut findings,
        );
        assert!(findings.into_dependencies().0.is_empty());
    }
}
