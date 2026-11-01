//! `perl.prov`, `perl.req` and `perllib.attr`.
//!
//! Classic RPM Perl generators are full lexers. This is a deliberately smaller
//! reader: it understands `package`, `$VERSION`, `use`/`require`/`no`, POD and
//! comments, which covers ordinary CPAN modules and scripts. It does not model
//! `eval`-generated `use` statements, here-docs or `q{}` blocks, so it can miss
//! a module a file loads indirectly.

use std::collections::{BTreeMap, BTreeSet};

use crate::package_spec::BinaryPackageFile;

use super::Findings;

#[derive(Default)]
pub struct Scanner {
    findings: Findings,
    provides: BTreeMap<String, Option<String>>,
    requires: BTreeMap<String, Option<String>>,
    raw_requires: BTreeSet<String>,
}

impl Scanner {
    pub fn inspect(&mut self, file: &BinaryPackageFile) {
        if !is_perl_source(file) {
            return;
        }
        let Ok(text) = std::str::from_utf8(&file.data) else {
            return;
        };
        let mut package = None;
        let mut in_pod = false;
        for raw in text.lines() {
            let line = strip_comment(raw);
            let trimmed = line.trim();
            if in_pod {
                if trimmed.starts_with("=cut") {
                    in_pod = false;
                }
                continue;
            }
            if trimmed.starts_with('=')
                && (trimmed.starts_with("=pod")
                    || trimmed.starts_with("=head")
                    || trimmed.starts_with("=over")
                    || trimmed.starts_with("=item")
                    || trimmed.starts_with("=for")
                    || trimmed.starts_with("=begin")
                    || trimmed.starts_with("=encoding"))
            {
                in_pod = true;
                continue;
            }
            if trimmed == "__END__" || trimmed == "__DATA__" {
                break;
            }
            if let Some((name, version)) = parse_package(trimmed) {
                self.provides
                    .entry(name.clone())
                    .and_modify(|current| {
                        if current.is_none() {
                            *current = version.clone();
                        }
                    })
                    .or_insert(version);
                package = Some(name);
                continue;
            }
            if let Some(version) = parse_version_assignment(trimmed) {
                if let Some(package) = &package {
                    self.provides.insert(package.clone(), Some(version));
                }
                continue;
            }
            self.parse_statement(trimmed);
        }
    }

    pub fn finish(mut self) -> Findings {
        for (module, version) in &self.provides {
            match version {
                Some(version) => self.findings.provide(format!("perl({module}) = {version}")),
                None => self.findings.provide(format!("perl({module})")),
            }
        }
        for (module, version) in &self.requires {
            if self.provides.contains_key(module) {
                continue;
            }
            match version {
                Some(version) => self
                    .findings
                    .require(format!("perl({module}) >= {version}")),
                None => self.findings.require(format!("perl({module})")),
            }
        }
        for dependency in std::mem::take(&mut self.raw_requires) {
            self.findings.require(dependency);
        }
        self.findings
    }

    fn parse_statement(&mut self, line: &str) {
        let Some(rest) = ["use ", "require ", "no "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        else {
            return;
        };
        let rest = rest.trim_start();
        if rest.starts_with('v')
            || rest
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
        {
            if let Some(version) = rest
                .trim_start_matches('v')
                .split(|character: char| !(character.is_ascii_digit() || character == '.'))
                .next()
            {
                if !version.is_empty() {
                    self.raw_requires
                        .insert(format!("perl(:VERSION) >= {}", perl_version(version)));
                }
            }
            return;
        }

        let end = rest
            .find(|character: char| {
                !(character.is_ascii_alphanumeric() || character == '_' || character == ':')
            })
            .unwrap_or(rest.len());
        let module = rest[..end].trim_end_matches(':');
        if !valid_module(module) || module == "main" {
            return;
        }
        let after = rest[end..].trim_start();
        let candidate = after.strip_prefix('v').unwrap_or(after);
        let version = candidate
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
            .then(|| {
                candidate
                    .split(|character: char| {
                        !(character.is_ascii_digit() || character == '.' || character == '_')
                    })
                    .next()
                    .unwrap_or(candidate)
                    .to_string()
            })
            .filter(|version| !version.is_empty());

        self.add_require(module, version);
        let takes_arguments = module == "base"
            || module == "aliased"
            || (module == "parent" && !after.contains("-norequire"));
        if takes_arguments {
            for argument in module_arguments(after) {
                self.add_require(&argument, None);
            }
        }
    }

    fn add_require(&mut self, module: &str, version: Option<String>) {
        self.requires
            .entry(module.to_string())
            .and_modify(|current| {
                if version.is_some()
                    && current.as_ref().is_none_or(|current| {
                        version
                            .as_ref()
                            .is_some_and(|version| version_gt(version, current))
                    })
                {
                    *current = version.clone();
                }
            })
            .or_insert(version);
    }
}

fn is_perl_source(file: &BinaryPackageFile) -> bool {
    if file.path.ends_with(".pm") {
        return true;
    }
    if file.mode & 0o111 == 0 || !file.data.starts_with(b"#!") {
        return false;
    }
    let first = file
        .data
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(file.data.len());
    std::str::from_utf8(&file.data[2..first])
        .ok()
        .and_then(|line| line.split_whitespace().next())
        .is_some_and(|interpreter| interpreter == "perl" || interpreter.ends_with("/perl"))
}

fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quote.is_some() => escaped = true,
            '\'' | '"' if quote == Some(character) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(character),
            '#' if quote.is_none() => return &line[..index],
            _ => {}
        }
    }
    line
}

fn parse_package(line: &str) -> Option<(String, Option<String>)> {
    let rest = line.strip_prefix("package ")?;
    let rest = rest.trim_start();
    let end = rest
        .find(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '_' || character == ':')
        })
        .unwrap_or(rest.len());
    let name = rest[..end].trim_end_matches(':');
    if !valid_module(name) || name == "main" {
        return None;
    }
    let after = rest[end..].trim_start();
    let version = after.split([';', '{']).next().unwrap_or("").trim();
    let version = version.strip_prefix('v').unwrap_or(version);
    let version = version.split_whitespace().next().unwrap_or("").to_string();
    Some((
        name.to_string(),
        (!version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.'))
            .then_some(version),
    ))
}

fn parse_version_assignment(line: &str) -> Option<String> {
    let (left, right) = line.split_once('=')?;
    if !left.trim_end().ends_with("VERSION") {
        return None;
    }
    let right = right.trim_start();
    let right = right
        .strip_prefix(['\'', '"'])
        .unwrap_or(right)
        .trim_start_matches('v');
    let end = right
        .find(|character: char| {
            !(character.is_ascii_digit() || character == '.' || character == '_')
        })
        .unwrap_or(right.len());
    let version = right[..end].trim_end_matches('_');
    (!version.is_empty()).then(|| version.to_string())
}

fn module_arguments(line: &str) -> Vec<String> {
    let mut modules = quoted_modules(line);
    if let Some(start) = line.find("qw") {
        let rest = line[start + 2..].trim_start();
        let Some(open) = rest.chars().next() else {
            return modules;
        };
        let close = match open {
            '(' => ')',
            '{' => '}',
            '[' => ']',
            '/' => '/',
            '!' => '!',
            '|' => '|',
            '\'' => '\'',
            '"' => '"',
            _ => return modules,
        };
        let Some(end) = rest[1..].find(close) else {
            return modules;
        };
        for module in rest[1..1 + end].split_whitespace() {
            let module = module.trim_end_matches(':');
            if valid_module(module) && module != "main" {
                modules.push(module.to_string());
            }
        }
    }
    modules
}

fn quoted_modules(line: &str) -> Vec<String> {
    let mut modules = Vec::new();
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\'' && character != '"' {
            continue;
        }
        let mut module = String::new();
        for character in characters.by_ref() {
            if character == '\'' || character == '"' {
                break;
            }
            module.push(character);
        }
        let module = module.trim().trim_end_matches(':');
        if valid_module(module) && module != "main" {
            modules.push(module.to_string());
        }
    }
    modules
}

fn valid_module(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == ':'
        })
        && !name.ends_with(':')
}

fn version_gt(left: &str, right: &str) -> bool {
    let components = |version: &str| {
        version
            .split('.')
            .map(|component| component.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let mut left = components(left);
    let mut right = components(right);
    let length = left.len().max(right.len());
    left.resize(length, 0);
    right.resize(length, 0);
    left > right
}

fn perl_version(version: &str) -> String {
    if !version.contains('.') && version.len() >= 3 && version.chars().all(|c| c.is_ascii_digit()) {
        format!("{}.{}", &version[..1], &version[1..])
    } else {
        version.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::test_file;

    fn inspect(path: &str, data: &str) -> (Vec<String>, Vec<String>) {
        let mut scanner = Scanner::default();
        scanner.inspect(&test_file(path, data.as_bytes()));
        scanner.finish().into_dependencies()
    }

    #[test]
    fn packages_and_versions_become_provides() {
        let (provides, _) = inspect(
            "/usr/share/perl5/Demo.pm",
            "package Demo;\nour $VERSION = '1.23';\n1;\n",
        );
        assert_eq!(provides, vec!["perl(Demo) = 1.23"]);
    }

    #[test]
    fn use_statements_become_requires_and_skip_self_provides() {
        let (_, requires) = inspect(
            "/usr/share/perl5/Demo.pm",
            "package Demo;\nuse strict;\nuse JSON::PP 4.0;\nuse Demo::Helper;\npackage Demo::Helper;\n",
        );
        assert!(requires.contains(&"perl(strict)".to_string()));
        assert!(requires.contains(&"perl(JSON::PP) >= 4.0".to_string()));
        assert!(!requires
            .iter()
            .any(|require| require.contains("Demo::Helper")));
    }

    #[test]
    fn comments_and_pod_are_ignored() {
        let (provides, requires) = inspect(
            "/usr/share/perl5/Demo.pm",
            "package Demo;\n# use Fake::Comment;\n=head1 NAME\n\nuse Fake::Pod;\n=cut\nuse Real::Module;\n",
        );
        assert_eq!(provides, vec!["perl(Demo)"]);
        assert!(!requires.iter().any(|require| require.contains("Fake")));
        assert!(requires.contains(&"perl(Real::Module)".to_string()));
    }

    #[test]
    fn use_base_and_parent_require_their_arguments() {
        let (_, requires) = inspect(
            "/usr/share/perl5/Demo.pm",
            "package Demo;\n\
             use base qw(Base::One Base::Two);\n\
             use parent -norequire, 'Parent::Class';\n",
        );
        assert!(requires.contains(&"perl(base)".to_string()));
        assert!(requires.contains(&"perl(Base::One)".to_string()));
        assert!(requires.contains(&"perl(Base::Two)".to_string()));
        assert!(requires.contains(&"perl(parent)".to_string()));
        assert!(!requires
            .iter()
            .any(|require| require.contains("Parent::Class")));
    }

    #[test]
    fn a_perl_shebang_script_is_scanned() {
        let (_, requires) = inspect("/usr/bin/demo", "#!/usr/bin/perl\nuse Getopt::Long;\n");
        assert_eq!(requires, vec!["perl(Getopt::Long)"]);
    }

    #[test]
    fn a_version_pragma_becomes_a_perl_version_require() {
        let (_, requires) = inspect("/usr/bin/demo", "#!/usr/bin/perl\nuse v5.36;\n");
        assert_eq!(requires, vec!["perl(:VERSION) >= 5.36"]);
    }
}
