use std::collections::BTreeMap;

use super::Findings;

/// `libsymlink.attr`: a development `.so` symlink requires the soname of the
/// library it points at, so a `-devel` output cannot install without its lib.
pub fn inspect(
    path: &str,
    target: &str,
    sonames: &BTreeMap<String, String>,
    findings: &mut Findings,
) {
    if !path.ends_with(".so") || !target.contains("lib") || !target.contains(".so.") {
        return;
    }
    let Some(dependency) = sonames.get(&resolve(path, target)) else {
        return;
    };
    if findings.contains_provide(dependency) {
        return;
    }
    findings.require(dependency.clone());
}

pub(super) fn resolve(path: &str, target: &str) -> String {
    let joined = if target.starts_with('/') {
        target.to_string()
    } else {
        match path.rsplit_once('/') {
            Some((directory, _)) => format!("{directory}/{target}"),
            None => target.to_string(),
        }
    };
    normalize(&joined)
}

fn normalize(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_target_resolves_next_to_the_symlink() {
        assert_eq!(
            resolve("/usr/lib64/libz.so", "libz.so.1.3.1"),
            "/usr/lib64/libz.so.1.3.1"
        );
    }

    #[test]
    fn parent_components_are_collapsed() {
        assert_eq!(
            resolve("/usr/lib64/libz.so", "../lib/libz.so.1"),
            "/usr/lib/libz.so.1"
        );
    }

    #[test]
    fn the_target_soname_becomes_a_require() {
        let mut sonames = BTreeMap::new();
        sonames.insert(
            "/usr/lib64/libz.so.1".to_string(),
            "libz.so.1()(64bit)".to_string(),
        );
        let mut findings = Findings::default();
        inspect("/usr/lib64/libz.so", "libz.so.1", &sonames, &mut findings);
        assert_eq!(findings.into_dependencies().1, vec!["libz.so.1()(64bit)"]);
    }

    #[test]
    fn a_symlink_to_a_bundled_library_needs_nothing() {
        let mut sonames = BTreeMap::new();
        sonames.insert(
            "/usr/lib64/libz.so.1".to_string(),
            "libz.so.1()(64bit)".to_string(),
        );
        let mut findings = Findings::default();
        findings.provide("libz.so.1()(64bit)");
        inspect("/usr/lib64/libz.so", "libz.so.1", &sonames, &mut findings);
        assert!(findings.into_dependencies().1.is_empty());
    }
}
