//! `desktop.attr`: application and MIME-handler provides from `.desktop` files.

use crate::package_spec::BinaryPackageFile;

use super::Findings;

pub fn inspect(file: &BinaryPackageFile, findings: &mut Findings) {
    let Some(name) = desktop_name(&file.path) else {
        return;
    };
    let Ok(text) = std::str::from_utf8(&file.data) else {
        return;
    };

    let mut executable = false;
    let mut mimetypes = Vec::new();
    for line in logical_lines(text) {
        if (line.starts_with("Type=") && line.trim_end() == "Type=Application")
            || line.starts_with("Exec=")
        {
            executable = true;
        } else if let Some(value) = line.strip_prefix("MimeType=") {
            mimetypes.extend(
                value
                    .split(';')
                    .map(str::trim)
                    .filter(|mimetype| !mimetype.is_empty())
                    .map(str::to_string),
            );
        }
    }
    if !executable {
        return;
    }
    findings.provide("application()");
    findings.provide(format!("application({name})"));
    for mimetype in mimetypes {
        findings.provide(format!("mimehandler({mimetype})"));
    }
}

fn desktop_name(path: &str) -> Option<&str> {
    if !path.contains("/applications/") || !path.ends_with(".desktop") {
        return None;
    }
    path.rsplit('/').next()
}

fn logical_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::test_file;

    #[test]
    fn an_application_provides_itself_and_its_mime_handlers() {
        let mut findings = Findings::default();
        inspect(
            &test_file(
                "/usr/share/applications/org.example.Tool.desktop",
                b"[Desktop Entry]\nType=Application\nExec=tool %U\nMimeType=text/plain;image/png;\n",
            ),
            &mut findings,
        );
        let (provides, requires) = findings.into_dependencies();
        assert_eq!(
            provides,
            vec![
                "application()",
                "application(org.example.Tool.desktop)",
                "mimehandler(image/png)",
                "mimehandler(text/plain)",
            ]
        );
        assert!(requires.is_empty());
    }

    #[test]
    fn a_link_entry_is_not_an_application() {
        let mut findings = Findings::default();
        inspect(
            &test_file(
                "/usr/share/applications/link.desktop",
                b"[Desktop Entry]\nType=Link\nURL=https://example.org\n",
            ),
            &mut findings,
        );
        assert!(findings.into_dependencies().0.is_empty());
    }
}
