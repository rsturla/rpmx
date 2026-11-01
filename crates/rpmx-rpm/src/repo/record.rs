use rpm::IndexTag;

use rpmx_core::Error;

fn split_evr(value: &str) -> (String, String, String) {
    let (epoch, rest) = match value.split_once(':') {
        Some((epoch, rest)) => (epoch, rest),
        None => ("0", value),
    };
    match rest.rsplit_once('-') {
        Some((version, release)) => (epoch.to_string(), version.to_string(), release.to_string()),
        None => (epoch.to_string(), rest.to_string(), String::new()),
    }
}

pub(super) fn record(
    package: &rpm::Package,
    filename: &str,
    checksum: &str,
    package_size: u64,
    primary: &mut String,
    filelists: &mut String,
    other: &mut String,
) -> Result<(), Error> {
    let name = package.metadata.get_name().unwrap_or("");
    let arch = package.metadata.get_arch().unwrap_or("");
    let version = package.metadata.get_version().unwrap_or("");
    let release = package.metadata.get_release().unwrap_or("");
    let epoch = package.metadata.get_epoch().unwrap_or(0);
    let summary = package.metadata.get_summary().unwrap_or("");
    let description = package.metadata.get_description().unwrap_or("");
    let url = package.metadata.get_url().unwrap_or("");
    let license = package.metadata.get_license().unwrap_or("");
    let vendor = package.metadata.get_vendor().unwrap_or("");
    let build_host = package.metadata.get_build_host().unwrap_or("");
    let source_rpm = package.metadata.get_source_rpm().unwrap_or("");
    let build_time = package.metadata.get_build_time().unwrap_or(0);
    let installed = package.metadata.get_installed_size().unwrap_or(0);
    let offsets = package.metadata.get_package_segment_offsets();
    let files = package.metadata.get_file_entries().unwrap_or_default();

    primary.push_str(&format!(
        "  <package type=\"rpm\">\n    <name>{}</name>\n    <arch>{}</arch>\n    <version epoch=\"{}\" ver=\"{}\" rel=\"{}\"/>\n    <checksum type=\"sha256\" pkgid=\"YES\">{}</checksum>\n    <summary>{}</summary>\n    <description>{}</description>\n    <packager></packager>\n    <url>{}</url>\n    <time file=\"{}\" build=\"{}\"/>\n    <size package=\"{}\" installed=\"{}\" archive=\"{}\"/>\n    <location href=\"{}\"/>\n    <format>\n      <rpm:license>{}</rpm:license>\n      <rpm:vendor>{}</rpm:vendor>\n      <rpm:group></rpm:group>\n      <rpm:buildhost>{}</rpm:buildhost>\n      <rpm:sourcerpm>{}</rpm:sourcerpm>\n      <rpm:header-range start=\"{}\" end=\"{}\"/>\n",
        escape(name),
        escape(arch),
        epoch,
        escape(version),
        escape(release),
        checksum,
        escape(summary),
        escape(description),
        escape(url),
        build_time,
        build_time,
        package_size,
        installed,
        package_size.saturating_sub(offsets.payload),
        escape(filename),
        escape(license),
        escape(vendor),
        escape(build_host),
        escape(source_rpm),
        offsets.header,
        offsets.payload.saturating_sub(1)
    ));
    for (element, names, flags, versions) in [
        (
            "provides",
            IndexTag::RPMTAG_PROVIDENAME,
            IndexTag::RPMTAG_PROVIDEFLAGS,
            IndexTag::RPMTAG_PROVIDEVERSION,
        ),
        (
            "requires",
            IndexTag::RPMTAG_REQUIRENAME,
            IndexTag::RPMTAG_REQUIREFLAGS,
            IndexTag::RPMTAG_REQUIREVERSION,
        ),
        (
            "recommends",
            IndexTag::RPMTAG_RECOMMENDNAME,
            IndexTag::RPMTAG_RECOMMENDFLAGS,
            IndexTag::RPMTAG_RECOMMENDVERSION,
        ),
        (
            "suggests",
            IndexTag::RPMTAG_SUGGESTNAME,
            IndexTag::RPMTAG_SUGGESTFLAGS,
            IndexTag::RPMTAG_SUGGESTVERSION,
        ),
        (
            "supplements",
            IndexTag::RPMTAG_SUPPLEMENTNAME,
            IndexTag::RPMTAG_SUPPLEMENTFLAGS,
            IndexTag::RPMTAG_SUPPLEMENTVERSION,
        ),
        (
            "enhances",
            IndexTag::RPMTAG_ENHANCENAME,
            IndexTag::RPMTAG_ENHANCEFLAGS,
            IndexTag::RPMTAG_ENHANCEVERSION,
        ),
        (
            "conflicts",
            IndexTag::RPMTAG_CONFLICTNAME,
            IndexTag::RPMTAG_CONFLICTFLAGS,
            IndexTag::RPMTAG_CONFLICTVERSION,
        ),
        (
            "obsoletes",
            IndexTag::RPMTAG_OBSOLETENAME,
            IndexTag::RPMTAG_OBSOLETEFLAGS,
            IndexTag::RPMTAG_OBSOLETEVERSION,
        ),
    ] {
        let names = package
            .metadata
            .header
            .get_entry_data_as_string_array(names)
            .unwrap_or_default();
        let flags = package
            .metadata
            .header
            .get_entry_data_as_u32_array(flags)
            .unwrap_or_default();
        let versions = package
            .metadata
            .header
            .get_entry_data_as_string_array(versions)
            .unwrap_or_default();
        if names.is_empty() {
            continue;
        }
        primary.push_str(&format!("      <rpm:{element}>\n"));
        for (index, name) in names.iter().enumerate() {
            let flag =
                rpm::DependencyFlags::from_bits_truncate(flags.get(index).copied().unwrap_or(0));
            let version = versions.get(index).copied().unwrap_or("");
            let comparator = flag.comparator_str();
            let kind = match comparator {
                "<" => "LT",
                "<=" => "LE",
                ">" => "GT",
                ">=" => "GE",
                "=" => "EQ",
                _ => "",
            };
            if version.is_empty() || kind.is_empty() {
                primary.push_str(&format!("        <rpm:entry name=\"{}\"/>\n", escape(name)));
            } else {
                let (dependency_epoch, dependency_version, dependency_release) = split_evr(version);
                let release = if dependency_release.is_empty() {
                    String::new()
                } else {
                    format!(" rel=\"{}\"", escape(&dependency_release))
                };
                primary.push_str(&format!(
                    "        <rpm:entry name=\"{}\" flags=\"{}\" epoch=\"{}\" ver=\"{}\"{}/>\n",
                    escape(name),
                    kind,
                    dependency_epoch,
                    escape(&dependency_version),
                    release
                ));
            }
        }
        primary.push_str(&format!("      </rpm:{element}>\n"));
    }
    for file in &files {
        let path = file.path();
        primary.push_str(&format!(
            "      <file>{}</file>\n",
            escape(&path.to_string_lossy())
        ));
    }
    primary.push_str("    </format>\n  </package>\n");

    filelists.push_str(&format!(
        "  <package pkgid=\"{}\" name=\"{}\" arch=\"{}\">\n    <version epoch=\"{}\" ver=\"{}\" rel=\"{}\"/>\n",
        checksum,
        escape(name),
        escape(arch),
        epoch,
        escape(version),
        escape(release)
    ));
    for file in &files {
        filelists.push_str(&format!(
            "    <file>{}</file>\n",
            escape(&file.path().to_string_lossy())
        ));
    }
    filelists.push_str("  </package>\n");

    other.push_str(&format!(
        "  <package pkgid=\"{}\" name=\"{}\" arch=\"{}\">\n    <version epoch=\"{}\" ver=\"{}\" rel=\"{}\"/>\n  </package>\n",
        checksum,
        escape(name),
        escape(arch),
        epoch,
        escape(version),
        escape(release)
    ));
    Ok(())
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}
