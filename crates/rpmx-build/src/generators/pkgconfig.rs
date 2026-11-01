use std::collections::BTreeMap;

use crate::package_spec::BinaryPackageFile;

use super::Findings;

/// `pkgconfig.attr`: `pkgconfig(<stem>)` provides and `pkgconfig(<name>)`
/// requires from `.pc` files.
///
/// The provide name is the file stem, not the `Name:` field. `libcrypt.pc`
/// declares `Name: libxcrypt` but provides `pkgconfig(libcrypt)`, matching
/// `pkg-config --print-provides`.
pub fn inspect(file: &BinaryPackageFile, findings: &mut Findings) {
    let Some(name) = stem(&file.path) else {
        return;
    };
    let Ok(text) = std::str::from_utf8(&file.data) else {
        return;
    };
    let fields = parse_fields(text);

    match fields
        .get("Version")
        .filter(|version| !version.contains('$'))
    {
        Some(version) => findings.provide(format!("pkgconfig({name}) = {version}")),
        None => findings.provide(format!("pkgconfig({name})")),
    }
    for key in ["Requires", "Requires.private"] {
        let Some(value) = fields.get(key) else {
            continue;
        };
        for dependency in parse_requires(value) {
            findings.require(dependency);
        }
    }
}

fn stem(path: &str) -> Option<&str> {
    let (directory, file) = path.rsplit_once('/')?;
    if !directory.ends_with("/pkgconfig") {
        return None;
    }
    file.strip_suffix(".pc")
}

fn parse_fields(text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut current: Option<(String, String)> = None;

    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some((_, value)) = current.as_mut() {
                if value.ends_with('\\') {
                    value.pop();
                } else {
                    value.push(' ');
                }
                value.push_str(line.trim());
            }
            continue;
        }
        if let Some((key, value)) = current.take() {
            fields.insert(key, value);
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let mut value = value.trim().to_string();
            if value.ends_with('\\') {
                value.pop();
            }
            current = Some((key.trim().to_string(), value));
        }
    }
    if let Some((key, value)) = current.take() {
        fields.insert(key, value);
    }
    fields
}

fn parse_requires(value: &str) -> Vec<String> {
    let mut dependencies = Vec::new();
    for item in value.split(',') {
        let words = item.split_whitespace().collect::<Vec<_>>();
        match words.as_slice() {
            [] => {}
            [name] => dependencies.push(format!("pkgconfig({name})")),
            [name, operator, version] if is_operator(operator) => {
                if version.is_empty() || version.contains('$') {
                    dependencies.push(format!("pkgconfig({name})"));
                } else {
                    dependencies.push(format!("pkgconfig({name}) {operator} {version}"));
                }
            }
            words => {
                for word in words {
                    if !is_operator(word)
                        && !word.starts_with(|character: char| character.is_ascii_digit())
                    {
                        dependencies.push(format!("pkgconfig({word})"));
                    }
                }
            }
        }
    }
    dependencies
}

fn is_operator(word: &str) -> bool {
    matches!(word, "=" | "<" | "<=" | ">" | ">=")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::test_file;

    fn findings(data: &str) -> (Vec<String>, Vec<String>) {
        let mut findings = Findings::default();
        inspect(
            &test_file("/usr/lib64/pkgconfig/libcrypt.pc", data.as_bytes()),
            &mut findings,
        );
        findings.into_dependencies()
    }

    #[test]
    fn the_provide_uses_the_file_stem_not_the_name_field() {
        let (provides, _) = findings("Name: libxcrypt\nVersion: 4.5.2\n");
        assert_eq!(provides, vec!["pkgconfig(libcrypt) = 4.5.2"]);
    }

    #[test]
    fn requires_and_requires_private_become_pkgconfig_requires() {
        let (_, requires) = findings(
            "Name: demo\nVersion: 1.0\nRequires: zlib >= 1.2, openssl\nRequires.private: libcrypto = 3.0\n",
        );
        assert!(requires.contains(&"pkgconfig(zlib) >= 1.2".to_string()));
        assert!(requires.contains(&"pkgconfig(openssl)".to_string()));
        assert!(requires.contains(&"pkgconfig(libcrypto) = 3.0".to_string()));
    }

    #[test]
    fn a_continuation_line_joins_the_previous_field() {
        let (_, requires) =
            findings("Name: demo\nVersion: 1.0\nRequires: zlib \\\n          openssl\n");
        assert!(requires.contains(&"pkgconfig(zlib)".to_string()));
        assert!(requires.contains(&"pkgconfig(openssl)".to_string()));
    }

    #[test]
    fn an_unexpanded_version_variable_is_left_unversioned() {
        let (provides, _) = findings("Name: demo\nVersion: ${version}\n");
        assert_eq!(provides, vec!["pkgconfig(libcrypt)"]);
    }
}
