//! Automatic `Provides` and `Requires` generation.
//!
//! Each submodule owns one file class and contributes dependencies from what
//! the package actually ships instead of trusting the recipe author. The
//! dispatcher runs
//! the ELF scanner first because a `.so` symlink needs the soname of the
//! library it points at, then the remaining scanners over every file.

use std::collections::{BTreeMap, BTreeSet};

use crate::package::StagedFile;
use crate::package_spec::BinaryPackageFile;
use rpmx_language::model::OutputGenerators;

mod debuginfo;
mod desktop;
mod elf;
mod libsymlink;
mod metainfo;
mod perl;
mod pkgconfig;
mod python;
mod script;

/// Dependency findings from one package output.
///
/// `local_paths` holds the regular files in this output. It is not written to
/// the RPM; it lets a generator suppress a self-require such as a Python
/// package that ships `/usr/bin/python3.12` and also uses it.
#[derive(Default)]
pub struct Findings {
    provides: BTreeSet<String>,
    requires: BTreeSet<String>,
    local_paths: BTreeSet<String>,
}

impl Findings {
    pub(crate) fn provide(&mut self, dependency: impl Into<String>) {
        self.provides.insert(dependency.into());
    }

    pub(crate) fn require(&mut self, dependency: impl Into<String>) {
        self.requires.insert(dependency.into());
    }

    pub(crate) fn provide_path(&mut self, path: &str) {
        self.local_paths.insert(path.to_string());
    }

    pub(crate) fn is_local_path(&self, path: &str) -> bool {
        self.local_paths.contains(path)
    }

    pub(crate) fn contains_provide(&self, dependency: &str) -> bool {
        self.provides.contains(dependency)
    }

    pub(crate) fn retain_requires(&mut self, keep: impl Fn(&str) -> bool) {
        self.requires.retain(|dependency| keep(dependency));
    }

    fn merge(&mut self, other: Findings) {
        self.provides.extend(other.provides);
        self.requires.extend(other.requires);
        self.local_paths.extend(other.local_paths);
    }

    fn exclude(&mut self, excludes: &[String]) {
        for dependency in excludes {
            self.provides.remove(dependency);
            self.requires.remove(dependency);
        }
    }

    fn into_dependencies(self) -> (Vec<String>, Vec<String>) {
        (
            self.provides.into_iter().collect(),
            self.requires.into_iter().collect(),
        )
    }
}

/// Map every staged library path to its soname dependency.
///
/// A `-devel` output holds only the `.so` symlink, so its target normally lives
/// in another output of the same package. The map is built once from the whole
/// stage and shared with every output.
pub fn sonames(files: &[StagedFile]) -> BTreeMap<String, String> {
    let mut sonames = BTreeMap::new();
    let mut targets = BTreeMap::new();
    for file in files {
        if let Some(target) = &file.target {
            targets.insert(file.path.clone(), target.clone());
        } else if let Some(info) = crate::elf::inspect(&file.data) {
            if let Some(soname) = info.soname {
                sonames.insert(
                    file.path.clone(),
                    crate::elf::soname_dependency(&soname, info.is_64),
                );
            }
        }
    }
    resolve_symlinks(&mut sonames, &targets);
    sonames
}

/// Give every symlink the soname of the library at the end of its chain.
///
/// Devel packages normally ship `libz.so -> libz.so.1`, and the library
/// package ships `libz.so.1 -> libz.so.1.3.1`. Looking only at the immediate
/// target would miss the soname.
fn resolve_symlinks(sonames: &mut BTreeMap<String, String>, targets: &BTreeMap<String, String>) {
    let mut changed = true;
    while changed {
        changed = false;
        for (path, target) in targets {
            if sonames.contains_key(path) {
                continue;
            }
            let resolved = libsymlink::resolve(path, target);
            if let Some(dependency) = sonames.get(&resolved).cloned() {
                sonames.insert(path.clone(), dependency);
                changed = true;
            }
        }
    }
}

/// Generate `(provides, requires)` for one package output.
///
/// `target_arch` is the build target, not the output arch: a `noarch` Python
/// wheel still evaluates environment markers against the architecture it was
/// built for. `sonames` covers the whole stage, so a symlink can find a
/// library that will be packaged in a different output.
pub fn generate(
    files: &[BinaryPackageFile],
    target_arch: &str,
    sonames: &BTreeMap<String, String>,
    options: &OutputGenerators,
) -> (Vec<String>, Vec<String>) {
    let mut findings = Findings::default();
    for file in files.iter().filter(|file| file.target.is_none()) {
        findings.provide_path(&file.path);
    }

    if options.elf {
        let mut elf = elf::Scanner::default();
        for file in files.iter().filter(|file| file.target.is_none()) {
            elf.inspect(file);
        }
        findings.merge(elf.finish());
    }

    let mut python = python::Scanner::default();
    let mut perl = perl::Scanner::default();
    for file in files {
        match &file.target {
            Some(target) => {
                if options.elf {
                    libsymlink::inspect(&file.path, target, sonames, &mut findings);
                }
            }
            None => {
                if options.script {
                    script::inspect(file, &mut findings);
                }
                if options.pkgconfig {
                    pkgconfig::inspect(file, &mut findings);
                }
                if options.python {
                    python.inspect(file, target_arch);
                }
                if options.perl {
                    perl.inspect(file);
                }
                if options.desktop {
                    desktop::inspect(file, &mut findings);
                }
                if options.metainfo {
                    metainfo::inspect(file, &mut findings);
                }
                if options.debuginfo {
                    debuginfo::inspect(file, &mut findings);
                }
            }
        }
    }
    findings.merge(python.finish());
    findings.merge(perl.finish());
    findings.exclude(&options.exclude);

    findings.into_dependencies()
}

#[cfg(test)]
pub(crate) fn test_file(path: &str, data: &[u8]) -> BinaryPackageFile {
    BinaryPackageFile {
        path: path.to_string(),
        data: data.to_vec(),
        mode: 0o755,
        target: None,
        user: None,
        group: None,
        caps: None,
        config: false,
        noreplace: false,
        doc: false,
        license: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkgconfig_file() -> BinaryPackageFile {
        test_file(
            "/usr/lib64/pkgconfig/demo.pc",
            b"Name: demo\nVersion: 1.0\n",
        )
    }

    #[test]
    fn a_disabled_generator_contributes_nothing() {
        let options = OutputGenerators {
            pkgconfig: false,
            ..OutputGenerators::default()
        };
        let (provides, requires) =
            generate(&[pkgconfig_file()], "x86_64", &BTreeMap::new(), &options);
        assert!(provides.is_empty(), "{provides:?}");
        assert!(requires.is_empty(), "{requires:?}");
    }

    #[test]
    fn symlink_chains_take_the_soname_of_the_real_library() {
        let mut sonames = BTreeMap::from([(
            "/usr/lib64/libz.so.1.3.1".to_string(),
            "libz.so.1()(64bit)".to_string(),
        )]);
        let targets = BTreeMap::from([
            ("/usr/lib64/libz.so".to_string(), "libz.so.1".to_string()),
            (
                "/usr/lib64/libz.so.1".to_string(),
                "libz.so.1.3.1".to_string(),
            ),
        ]);
        resolve_symlinks(&mut sonames, &targets);
        assert_eq!(
            sonames.get("/usr/lib64/libz.so"),
            Some(&"libz.so.1()(64bit)".to_string())
        );
        assert_eq!(
            sonames.get("/usr/lib64/libz.so.1"),
            Some(&"libz.so.1()(64bit)".to_string())
        );
    }

    #[test]
    fn exclude_drops_a_generated_provide_or_require_by_exact_match() {
        let options = OutputGenerators {
            exclude: vec!["pkgconfig(demo) = 1.0".to_string()],
            ..OutputGenerators::default()
        };
        let (provides, requires) =
            generate(&[pkgconfig_file()], "x86_64", &BTreeMap::new(), &options);
        assert!(provides.is_empty(), "{provides:?}");
        assert!(requires.is_empty(), "{requires:?}");
    }
}
