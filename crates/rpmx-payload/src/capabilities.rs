//! Match RPM requirements against package headers without resolving repositories.
use std::cmp::Ordering;
use std::collections::BTreeMap;

use rpm::{Dependency, DependencyFlags as Flags};
use rpmx_base::error::{codes, Error, Result};
use rpmx_base::version::compare;

use crate::metadata::PackageMetadata;

#[derive(Default)]
pub struct Capabilities(BTreeMap<String, Vec<Dependency>>);

impl Capabilities {
    pub fn insert(&mut self, package: &PackageMetadata) {
        self.extend(
            package
                .provides
                .iter()
                .cloned()
                .chain([Dependency::eq(&package.name, package.evr())])
                .chain(package.files.iter().map(Dependency::any)),
        );
    }

    pub fn satisfies(&self, requirement: &str) -> Result<bool> {
        let required = parse_dependency(requirement)?;
        Ok(self.0.get(&required.name).is_some_and(|providers| {
            providers
                .iter()
                .any(|provided| overlaps(provided, &required))
        }))
    }
}

impl Extend<Dependency> for Capabilities {
    fn extend<T: IntoIterator<Item = Dependency>>(&mut self, provides: T) {
        for provide in provides {
            self.0
                .entry(provide.name.clone())
                .or_default()
                .push(provide);
        }
    }
}

/// Parse the simple RPM dependency syntax shared by packaging and validation.
pub fn parse_dependency(text: &str) -> Result<rpm::Dependency> {
    let text = text.trim();
    if text.starts_with('(') {
        return Err(Error::new(
            codes::RPM_012,
            format!("rich dependency '{text}' is not supported yet"),
        ));
    }
    let parts: Vec<&str> = text.split_whitespace().collect();
    match parts.as_slice() {
        [name] => Ok(rpm::Dependency::any(*name)),
        [name, "=", version] => Ok(rpm::Dependency::eq(*name, *version)),
        [name, ">=", version] => Ok(rpm::Dependency::greater_eq(*name, *version)),
        [name, ">", version] => Ok(rpm::Dependency::greater(*name, *version)),
        [name, "<=", version] => Ok(rpm::Dependency::less_eq(*name, *version)),
        [name, "<", version] => Ok(rpm::Dependency::less(*name, *version)),
        _ => Err(Error::new(
            codes::RPM_013,
            format!("cannot parse dependency '{text}'"),
        )),
    }
}

// Follow rpm's rpmdsCompare/rpmverOverlap, including existence provides and
// omitted releases. This compares ranges, not complete package EVRs.
// https://github.com/rpm-software-management/rpm/blob/master/rpmio/rpmver.cc
fn overlaps(provided: &Dependency, required: &Dependency) -> bool {
    let mask = Flags::LESS | Flags::EQUAL | Flags::GREATER;
    let a = provided.flags & mask;
    let b = required.flags & mask;
    if a.is_empty() || b.is_empty() || provided.version.is_empty() || required.version.is_empty() {
        return true;
    }
    let (ae, av, ar) = split_evr(&provided.version);
    let (be, bv, br) = split_evr(&required.version);
    let mut order = compare(ae, be).then_with(|| compare(av, bv));
    if order == Ordering::Equal {
        if !ar.is_empty() && !br.is_empty() {
            order = compare(ar, br);
        } else if (!ar.is_empty() && b.contains(Flags::EQUAL))
            || (!br.is_empty() && a.contains(Flags::EQUAL))
        {
            return true;
        }
    }
    match order {
        Ordering::Less => a.contains(Flags::GREATER) || b.contains(Flags::LESS),
        Ordering::Greater => a.contains(Flags::LESS) || b.contains(Flags::GREATER),
        Ordering::Equal => !(a & b).is_empty(),
    }
}

fn split_evr(value: &str) -> (&str, &str, &str) {
    let (epoch, rest) = match value.split_once(':') {
        Some((epoch, rest)) if epoch.bytes().all(|byte| byte.is_ascii_digit()) => {
            (if epoch.is_empty() { "0" } else { epoch }, rest)
        }
        _ => ("0", value),
    };
    let (version, release) = rest.rsplit_once('-').unwrap_or((rest, ""));
    (epoch, version, release)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn headers_supply_names_virtual_capabilities_and_files() {
        let mut capabilities = Capabilities::default();
        capabilities.insert(&PackageMetadata {
            name: "compiler-16-default".into(),
            arch: "x86_64".into(),
            epoch: 1,
            version: "16.2".into(),
            release: "3".into(),
            provides: vec![
                Dependency::eq("gcc-c++", "1:16.2-3"),
                Dependency::eq("pkgconfig(compiler)", "2.0"),
                Dependency::any("libcompiler.so.4()(64bit)"),
            ],
            requires: vec![],
            files: BTreeSet::from(["/usr/bin/g++".into()]),
        });
        for requirement in [
            "compiler-16-default = 1:16.2",
            "gcc-c++ >= 1:16.0",
            "pkgconfig(compiler) = 2.0",
            "/usr/bin/g++",
            "libcompiler.so.4()(64bit)",
        ] {
            assert!(
                capabilities.satisfies(requirement).unwrap(),
                "{requirement}"
            );
        }
        for requirement in [
            "gcc-c++ >= 2:1",
            "gcc-c++ < 1:16.2",
            "pkgconfig(compiler) > 2.0",
            "libcompiler.so.3()(64bit)",
            "missing",
        ] {
            assert!(
                !capabilities.satisfies(requirement).unwrap(),
                "{requirement}"
            );
        }
        for requirement in ["(gcc or clang)", "gcc != 16", "gcc >="] {
            assert!(capabilities.satisfies(requirement).is_err());
        }
    }

    #[test]
    fn dependency_ranges_follow_rpm_epoch_release_and_existence_rules() {
        for (provided, required, expected) in [
            (Dependency::eq("x", "2-3"), Dependency::eq("x", "2"), true),
            (
                Dependency::eq("x", "2-3"),
                Dependency::greater("x", "2"),
                false,
            ),
            (
                Dependency::eq("x", "2-3"),
                Dependency::less("x", "2"),
                false,
            ),
            (
                Dependency::eq("x", "2-3"),
                Dependency::greater_eq("x", "2"),
                true,
            ),
            (
                Dependency::eq("x", "2-3"),
                Dependency::less_eq("x", "2"),
                true,
            ),
            (
                Dependency::eq("x", "1:2-3"),
                Dependency::eq("x", "2-3"),
                false,
            ),
            (
                Dependency::eq("x", "0:2-3"),
                Dependency::eq("x", "2-3"),
                true,
            ),
            (
                Dependency::eq("x", "2~rc1"),
                Dependency::less("x", "2"),
                true,
            ),
            (
                Dependency::eq("x", "2^git1"),
                Dependency::greater("x", "2"),
                true,
            ),
            (
                Dependency::greater("x", "2"),
                Dependency::less("x", "2"),
                false,
            ),
            (
                Dependency::greater("x", "2"),
                Dependency::less("x", "3"),
                true,
            ),
            (Dependency::any("x"), Dependency::greater("x", "3"), true),
        ] {
            assert_eq!(
                overlaps(&provided, &required),
                expected,
                "{provided:?}, {required:?}"
            );
        }
    }
}

#[cfg(test)]
mod conformance {
    use super::*;
    use std::process::Command;

    #[test]
    #[ignore = "requires /usr/bin/python3 with the native rpm bindings"]
    fn dependency_ranges_match_native_rpm() {
        let output = Command::new("/usr/bin/python3")
            .args([
                "-I",
                "-c",
                r#"
import itertools
import rpm
versions = ['', '0:2', '2', '2-1', '2-3', '1:2-3', '2~rc1', '2^git1', '3', '00:2', ':2', '2-']
flags = [0, rpm.RPMSENSE_LESS, rpm.RPMSENSE_GREATER, rpm.RPMSENSE_EQUAL,
         rpm.RPMSENSE_LESS | rpm.RPMSENSE_EQUAL, rpm.RPMSENSE_GREATER | rpm.RPMSENSE_EQUAL]
for af, av, bf, bv in itertools.product(flags, versions, flags, versions):
    a = rpm.ds(('virtual', af, av), 'provides')
    b = rpm.ds(('virtual', bf, bv), 'requires')
    next(a)
    next(b)
    print(af, av, bf, bv, int(a.Compare(b)), sep='\t')
"#,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for row in String::from_utf8(output.stdout).unwrap().lines() {
            let fields = row.split('\t').collect::<Vec<_>>();
            let provided = Dependency {
                name: "virtual".into(),
                flags: Flags::from_bits_retain(fields[0].parse().unwrap()),
                version: fields[1].into(),
            };
            let required = Dependency {
                name: "virtual".into(),
                flags: Flags::from_bits_retain(fields[2].parse().unwrap()),
                version: fields[3].into(),
            };
            assert_eq!(overlaps(&provided, &required), fields[4] == "1", "{row}");
        }
    }
}
