//! PEP 440 to RPM version conversion.
//!
//! RPM Python generators rewrite Python versions so RPM ordering matches
//! PEP 440: pre-releases sort before the release (`~`), post-releases after it
//! (`^`), and development releases before pre-releases (`~~`).

/// Convert a PEP 440 version to the form RPM Python generators write.
pub fn to_rpm(version: &str) -> String {
    let version = version.trim();
    if version.is_empty() {
        return String::new();
    }
    let version = version.strip_prefix(['v', 'V']).unwrap_or(version);
    let (epoch, rest) = split_epoch(version);
    let rest = rest.split('+').next().unwrap_or(rest);
    let (release, suffix) = split_release(rest);
    let release = normalize_release(release);
    if release.is_empty() {
        return version.to_string();
    }

    let mut out = String::new();
    if let Some(epoch) = epoch {
        out.push_str(epoch);
        out.push(':');
    }
    out.push_str(&release);
    out.push_str(&suffix);
    out
}

/// Turn a PEP 440 specifier list into RPM dependencies.
///
/// Multiple constraints stay as separate `Requires` entries; RPM ANDs them,
/// which avoids rich dependencies.
pub fn requirements(name: &str, specifiers: &str) -> Vec<String> {
    let mut dependencies = Vec::new();
    for specifier in specifiers.split(',') {
        let specifier = specifier.trim();
        if specifier.is_empty() {
            continue;
        }
        let Some((operator, version)) = split_operator(specifier) else {
            continue;
        };
        if version.is_empty() {
            continue;
        }
        match operator {
            "==" => {}
            "!=" => continue,
            "===" => {
                dependencies.push(format!("{name} = {version}"));
                continue;
            }
            "~=" => {
                dependencies.push(format!("{name} >= {}", to_rpm(version)));
                if let Some(upper) = compatible_upper(version) {
                    dependencies.push(format!("{name} < {}", to_rpm(&upper)));
                }
                continue;
            }
            "<=" | ">=" | "<" | ">" => {
                dependencies.push(format!("{name} {operator} {}", to_rpm(version)));
                continue;
            }
            _ => continue,
        }

        if let Some(prefix) = version.strip_suffix(".*") {
            dependencies.push(format!("{name} >= {}", to_rpm(prefix)));
            if let Some(upper) = increment_last(prefix) {
                dependencies.push(format!("{name} < {}", to_rpm(&upper)));
            }
        } else {
            dependencies.push(format!("{name} = {}", to_rpm(version)));
        }
    }
    dependencies
}

fn split_epoch(version: &str) -> (Option<&str>, &str) {
    match version.split_once('!') {
        Some((epoch, rest))
            if !epoch.is_empty() && epoch.chars().all(|character| character.is_ascii_digit()) =>
        {
            (Some(epoch), rest)
        }
        _ => (None, version),
    }
}

fn split_release(version: &str) -> (&str, String) {
    let end = version
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(version.len());
    let (release, suffix) = version.split_at(end);
    (release, suffix_to_rpm(suffix))
}

fn normalize_release(release: &str) -> String {
    let mut components = release
        .split('.')
        .filter(|component| !component.is_empty())
        .map(|component| component.trim_start_matches('0'))
        .map(|component| {
            if component.is_empty() {
                "0".to_string()
            } else {
                component.to_string()
            }
        })
        .collect::<Vec<_>>();
    while components.len() > 1 && components.last().is_some_and(|component| component == "0") {
        components.pop();
    }
    components.join(".")
}

fn suffix_to_rpm(suffix: &str) -> String {
    let suffix = suffix
        .trim_start_matches(['.', '-', '_'])
        .to_ascii_lowercase();
    if suffix.is_empty() {
        return String::new();
    }

    let mut pre = None;
    let mut post = None;
    let mut dev = None;
    for segment in suffix.split(['.', '-', '_']) {
        let Some((tag, number)) = split_tag(segment) else {
            continue;
        };
        match tag {
            "a" | "alpha" => pre = Some(("a", number)),
            "b" | "beta" => pre = Some(("b", number)),
            "c" | "rc" | "pre" | "preview" => pre = Some(("rc", number)),
            "post" | "rev" | "r" => post = Some(number),
            "dev" => dev = Some(number),
            _ => {}
        }
    }
    if let Some((tag, number)) = pre {
        format!("~{tag}{number}")
    } else if let Some(number) = dev {
        format!("~~dev{number}")
    } else if let Some(number) = post {
        format!("^post{number}")
    } else {
        String::new()
    }
}

fn split_tag(segment: &str) -> Option<(&str, String)> {
    if segment.is_empty() {
        return None;
    }
    let end = segment
        .find(|character: char| !character.is_ascii_alphabetic())
        .unwrap_or(segment.len());
    let (tag, digits) = segment.split_at(end);
    if tag.is_empty() {
        // A bare number after a separator is the legacy `1.0-1` post-release.
        if !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit()) {
            return Some(("post", digits.trim_start_matches('0').to_string()));
        }
        return None;
    }
    if !digits.is_empty() && !digits.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let number = if digits.is_empty() {
        "0".to_string()
    } else {
        digits.trim_start_matches('0').to_string()
    };
    Some((
        tag,
        if number.is_empty() {
            "0".to_string()
        } else {
            number
        },
    ))
}

fn split_operator(specifier: &str) -> Option<(&str, &str)> {
    for operator in ["===", "==", "!=", "~=", "<=", ">=", "<", ">"] {
        if let Some(version) = specifier.strip_prefix(operator) {
            return Some((operator, version.trim()));
        }
    }
    None
}

fn compatible_upper(version: &str) -> Option<String> {
    let (epoch, rest) = split_epoch(version);
    let components = release_components(rest)?;
    if components.len() < 2 {
        return None;
    }
    let mut upper = components[..components.len() - 1].to_vec();
    *upper.last_mut()? += 1;
    Some(with_epoch(epoch, &upper))
}

fn increment_last(version: &str) -> Option<String> {
    let (epoch, rest) = split_epoch(version);
    let mut components = release_components(rest)?;
    *components.last_mut()? += 1;
    Some(with_epoch(epoch, &components))
}

fn release_components(version: &str) -> Option<Vec<u64>> {
    let release = version.split(['-', '+']).next().unwrap_or(version);
    if release.is_empty() {
        return None;
    }
    release
        .split('.')
        .map(|component| component.parse::<u64>().ok())
        .collect()
}

fn with_epoch(epoch: Option<&str>, components: &[u64]) -> String {
    let release = components
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".");
    match epoch {
        Some(epoch) => format!("{epoch}:{release}"),
        None => release,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_drop_trailing_zero_components() {
        assert_eq!(to_rpm("1.0.0"), "1");
        assert_eq!(to_rpm("1.2.3"), "1.2.3");
        assert_eq!(to_rpm("1.0"), "1");
    }

    #[test]
    fn pre_releases_sort_before_the_release() {
        assert_eq!(to_rpm("1.0rc1"), "1~rc1");
        assert_eq!(to_rpm("1.0a1"), "1~a1");
        assert_eq!(to_rpm("1.0b2"), "1~b2");
        assert_eq!(to_rpm("1.0.preview1"), "1~rc1");
    }

    #[test]
    fn post_releases_sort_after_the_release() {
        assert_eq!(to_rpm("1.0.post1"), "1^post1");
        assert_eq!(to_rpm("1.0-1"), "1^post1");
    }

    #[test]
    fn development_releases_sort_before_pre_releases() {
        assert_eq!(to_rpm("1.0.dev1"), "1~~dev1");
    }

    #[test]
    fn epochs_and_local_versions_are_handled() {
        assert_eq!(to_rpm("1!2.0"), "1:2");
        assert_eq!(to_rpm("1.0+local.1"), "1");
        assert_eq!(to_rpm("v1.2.0"), "1.2");
    }

    #[test]
    fn equality_specifiers_become_exact_requires() {
        assert_eq!(
            requirements("python3.12dist(foo)", "==2.0.0"),
            vec!["python3.12dist(foo) = 2"]
        );
    }

    #[test]
    fn compatible_and_prefix_specifiers_get_an_upper_bound() {
        assert_eq!(
            requirements("python3.12dist(foo)", "~=1.4.5"),
            vec!["python3.12dist(foo) >= 1.4.5", "python3.12dist(foo) < 1.5"]
        );
        assert_eq!(
            requirements("python3.12dist(foo)", "~=1.4"),
            vec!["python3.12dist(foo) >= 1.4", "python3.12dist(foo) < 2"]
        );
        assert_eq!(
            requirements("python3.12dist(foo)", "==1.2.*"),
            vec!["python3.12dist(foo) >= 1.2", "python3.12dist(foo) < 1.3"]
        );
    }

    #[test]
    fn ranged_specifiers_keep_the_operator() {
        assert_eq!(
            requirements("python3.12dist(foo)", ">=1.0,<2.0"),
            vec!["python3.12dist(foo) >= 1", "python3.12dist(foo) < 2"]
        );
    }

    #[test]
    fn negative_specifiers_are_not_expressible_as_requires() {
        assert!(requirements("python3.12dist(foo)", "!=1.0").is_empty());
    }

    #[test]
    fn malformed_empty_versions_are_skipped() {
        assert!(requirements("python3.12dist(foo)", ">=").is_empty());
    }
}
