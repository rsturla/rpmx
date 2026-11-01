//! `metainfo.attr`: AppStream metadata provides.

use crate::package_spec::BinaryPackageFile;

use super::Findings;

pub fn inspect(file: &BinaryPackageFile, findings: &mut Findings) {
    let Some(name) = metainfo_name(&file.path) else {
        return;
    };
    findings.provide("metainfo()");
    findings.provide(format!("metainfo({name})"));
}

fn metainfo_name(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    let (directory, _) = path.rsplit_once('/')?;
    if !(directory.ends_with("/appdata") || directory.ends_with("/metainfo")) {
        return None;
    }
    if name.ends_with(".appdata.xml") || name.ends_with(".metainfo.xml") {
        Some(name)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::test_file;

    #[test]
    fn appstream_files_provide_a_metainfo_name() {
        let mut findings = Findings::default();
        inspect(
            &test_file(
                "/usr/share/metainfo/org.example.Tool.metainfo.xml",
                b"<component/>",
            ),
            &mut findings,
        );
        assert_eq!(
            findings.into_dependencies().0,
            vec!["metainfo()", "metainfo(org.example.Tool.metainfo.xml)"]
        );
    }

    #[test]
    fn an_unrelated_xml_file_is_ignored() {
        let mut findings = Findings::default();
        inspect(
            &test_file("/usr/share/metainfo/README.xml", b"<component/>"),
            &mut findings,
        );
        assert!(findings.into_dependencies().0.is_empty());
    }
}
