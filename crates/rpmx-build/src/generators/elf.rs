use crate::elf as parser;
use crate::package_spec::BinaryPackageFile;

use super::Findings;

/// ELF sonames and symbol versions, as `elfdeps` computes them.
#[derive(Default)]
pub struct Scanner {
    findings: Findings,
}

impl Scanner {
    pub fn inspect(&mut self, file: &BinaryPackageFile) {
        let Some(info) = parser::inspect(&file.data) else {
            return;
        };
        if let Some(soname) = &info.soname {
            let dependency = parser::soname_dependency(soname, info.is_64);
            self.findings.provide(dependency);
            for version in &info.version_defs {
                if version == soname {
                    continue;
                }
                self.findings
                    .provide(parser::versioned_dependency(soname, version, info.is_64));
            }
        }
        for needed in &info.needed {
            self.findings
                .require(parser::soname_dependency(needed, info.is_64));
        }
        for (library, version) in &info.version_needs {
            self.findings
                .require(parser::versioned_dependency(library, version, info.is_64));
        }
    }

    /// Local libraries satisfy their SONAME and symbol-version requirements
    /// even when the output does not export those capabilities publicly.
    pub fn finish(mut self) -> Findings {
        let provided = &self.findings.provides;
        self.findings
            .requires
            .retain(|dependency| !provided.contains(dependency));
        self.findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::test_file;

    #[test]
    fn a_script_is_not_an_elf_object() {
        let mut scanner = Scanner::default();
        scanner.inspect(&test_file("/usr/bin/tool", b"#!/bin/sh\n"));
        let (provides, requires) = scanner.finish().into_dependencies();
        assert!(provides.is_empty());
        assert!(requires.is_empty());
    }
}
