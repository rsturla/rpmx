use super::Ecosystem;

fn safe_version_input(version: &str) -> bool {
    version.is_ascii()
        && !version.chars().any(|ch| {
            ch.is_control()
                || ch.is_whitespace()
                || matches!(ch, '=' | '(' | ')' | '/' | ':' | '[' | ']' | '-')
        })
}

pub(crate) fn plain_version(version: &str) -> String {
    let version = version.trim();
    if safe_version_input(version) {
        version.to_string()
    } else {
        String::new()
    }
}

pub(crate) fn valid_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.is_ascii()
        && !name
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '(' | ')' | '='))
}

pub(crate) fn purl(ecosystem: Ecosystem, name: &str, version: &str) -> String {
    match ecosystem {
        Ecosystem::Golang => format!("pkg:golang/{name}@{version}"),
        Ecosystem::Cargo => format!("pkg:cargo/{name}@{version}"),
        Ecosystem::Npm => format!("pkg:npm/{name}@{version}"),
        Ecosystem::Python => format!("pkg:pypi/{name}@{version}"),
        Ecosystem::Ruby => format!("pkg:gem/{name}@{version}"),
        Ecosystem::Php => format!("pkg:composer/{name}@{version}"),
        Ecosystem::Generic => format!("pkg:generic/{name}@{version}"),
    }
}

pub fn go_version(version: &str) -> String {
    let version = version.trim();
    if !version.is_ascii() || version.chars().any(char::is_control) {
        return String::new();
    }
    // A Go pseudo-version ends in -<14-digit timestamp>-<commit>. Only that
    // shape uses the commit hash; length alone also matches legitimate long
    // tags such as v10.20.30-beta.1+incompatible.
    if let Some((rest, commit)) = version.rsplit_once('-') {
        if commit.len() >= 7 && commit.chars().all(|ch| ch.is_ascii_hexdigit()) {
            if let Some((_, timestamp)) = rest.rsplit_once('-') {
                if timestamp.len() == 14 && timestamp.chars().all(|ch| ch.is_ascii_digit()) {
                    return commit.chars().take(7).collect();
                }
            }
        }
    }
    version.trim_start_matches('v').replace('-', "_")
}

fn normalize_python_separators(version: &str) -> String {
    // PEP 440 treats '-', '_' and '.' as equivalent separators around a
    // pre/post marker. A trailing -NUM is the legacy post-release spelling.
    if let Some((base, rest)) = version.rsplit_once('-') {
        if !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()) {
            return format!("{base}.post{rest}");
        }
    }
    version.replace('-', ".")
}

pub fn python_version(version: &str) -> String {
    let version = version.trim();
    if !version.is_ascii()
        || version.chars().any(|ch| {
            ch.is_control()
                || ch.is_whitespace()
                || matches!(ch, '=' | '(' | ')' | '/' | ':' | '[' | ']')
        })
    {
        return String::new();
    }
    let version = version.split('+').next().unwrap_or(version);
    let version = normalize_python_separators(version);
    let (base, pre, dev, post) = python_parts(&version);
    let base = base.trim_end_matches('.');
    let mut parts = base.split('.').collect::<Vec<_>>();
    while parts.len() > 1 && parts.last() == Some(&"0") {
        parts.pop();
    }
    let base = parts.join(".");
    if let Some(pre) = pre {
        // Fedora's RpmVersion prefers the pre-release marker when both a
        // pre-release and a development marker are present.
        format!("{base}~{pre}")
    } else if let Some(dev) = dev {
        format!("{base}~~dev{dev}")
    } else if let Some(post) = post {
        format!("{base}^post{post}")
    } else {
        base
    }
}

fn python_parts(version: &str) -> (&str, Option<String>, Option<&str>, Option<&str>) {
    let (without_dev, dev) = version
        .split_once(".dev")
        .map_or((version, None), |(base, dev)| (base, Some(dev)));
    let (without_post, post) = without_dev
        .split_once(".post")
        .map_or((without_dev, None), |(base, post)| (base, Some(post)));
    let (base, pre) = split_prerelease(without_post).unwrap_or((without_post, None));
    (base, pre, dev, post)
}

fn split_prerelease(version: &str) -> Option<(&str, Option<String>)> {
    for marker in ["rc", "alpha", "beta", "a", "b"] {
        if let Some(index) = version.rfind(marker) {
            let rest = &version[index + marker.len()..];
            if !rest.is_empty()
                && rest.chars().all(|ch| ch.is_ascii_digit())
                && version[..index]
                    .chars()
                    .last()
                    .is_some_and(|ch| ch.is_ascii_digit() || ch == '.')
            {
                let marker = match marker {
                    "alpha" => "a",
                    "beta" => "b",
                    other => other,
                };
                return Some((&version[..index], Some(format!("{marker}{rest}"))));
            }
        }
    }
    None
}

pub fn ruby_version(version: &str) -> String {
    let version = version.trim();
    // Gem::Platform is appended after the version in lockfile entries, e.g.
    // 1.15.4-x86_64-linux; the package version is only the numeric part.
    let version = version
        .split_once('-')
        .map_or(version, |(version, _)| version);
    if !safe_version_input(version) {
        return String::new();
    }
    let mut release = Vec::new();
    let mut pre = Vec::new();
    for part in version.split('.') {
        if pre.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()) {
            release.push(part);
        } else {
            pre.push(part);
        }
    }
    if release.is_empty() {
        String::new()
    } else if pre.is_empty() {
        version.to_string()
    } else {
        format!("{}~{}", release.join("."), pre.join("."))
    }
}

pub fn cargo_version(version: &str) -> String {
    let version = version.trim();
    if !version.is_ascii() || version.chars().any(char::is_control) {
        return String::new();
    }
    let (without_build, build) = version.split_once('+').unwrap_or((version, ""));
    let (core, pre) = match without_build.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (without_build, None),
    };
    let mut rpm = match pre {
        Some(pre) => format!("{core}~{}", pre.replace('-', "_")),
        None => core.to_string(),
    };
    // cargo2rpm drops ordinary semver build metadata but preserves the git
    // reference it appends for a vendored git snapshot.
    if build.starts_with("git") {
        rpm.push('+');
        rpm.push_str(build);
    }
    rpm
}

#[cfg(test)]
mod tests {
    use super::{cargo_version, go_version, python_version, ruby_version};

    #[test]
    fn go_versions_follow_the_fedora_generator() {
        assert_eq!(go_version("v1.2.3"), "1.2.3");
        assert_eq!(go_version("v1.2.3-rc1"), "1.2.3_rc1");
        assert_eq!(go_version("v0.0.0-20240101000000-abcdef123456"), "abcdef1");
        assert_eq!(
            go_version("v10.20.30-beta.1+incompatible"),
            "10.20.30_beta.1+incompatible"
        );
        assert_eq!(go_version("v1.2.3-ααααααααααααααααα"), "");
    }

    #[test]
    fn cargo_versions_follow_cargo2rpm() {
        assert_eq!(cargo_version("1.2.3"), "1.2.3");
        assert_eq!(cargo_version("0.2.0-rc.0"), "0.2.0~rc.0");
        assert_eq!(cargo_version("0.2.0-rc.0+build.1"), "0.2.0~rc.0");
        assert_eq!(cargo_version("0.7.0+git20d2dacd"), "0.7.0+git20d2dacd");
    }

    #[test]
    fn python_versions_follow_the_fedora_shape() {
        assert_eq!(python_version("26.0"), "26");
        assert_eq!(python_version("1.0.0"), "1");
        assert_eq!(python_version("1.0rc1"), "1~rc1");
        assert_eq!(python_version("1.0alpha1"), "1~a1");
        assert_eq!(python_version("1.0.dev1"), "1~~dev1");
        assert_eq!(python_version("1.0.post1"), "1^post1");
        assert_eq!(python_version("1.0rc1.dev1"), "1~rc1");
        assert_eq!(python_version("1.0.a1"), "1~a1");
        assert_eq!(python_version("1.0-a1"), "1~a1");
        assert_eq!(python_version("1.0-alpha1"), "1~a1");
        assert_eq!(python_version("1.0-1"), "1^post1");
        assert_eq!(python_version("1.0 (beta)"), "");
    }

    #[test]
    fn ruby_prereleases_use_a_tilde() {
        assert_eq!(ruby_version("13.3.1"), "13.3.1");
        assert_eq!(ruby_version("1.14.0.rc1"), "1.14.0~rc1");
        assert_eq!(ruby_version("1.0.beta.1"), "1.0~beta.1");
        assert_eq!(ruby_version("1.15.4-x86_64-linux"), "1.15.4");
        assert_eq!(ruby_version("MyGem::VERSION"), "");
        assert_eq!(ruby_version("beta"), "");
    }
}
