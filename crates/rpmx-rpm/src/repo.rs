//! Minimal, deterministic RPM repository metadata.
//!
//! The generated layout is what dnf/libdnf consume from a `file://` or HTTPS
//! repository: `repodata/repomd.xml`, primary, filelists and other metadata.
//! With a signing key the command also writes `repodata/repomd.xml.asc`.

use std::path::{Path, PathBuf};

use rpm::IndexTag;
use rpmx_core::error::codes;
use rpmx_core::Error;
use sha2::{Digest, Sha256};

use crate::sign::SigningKey;

pub struct RepoOptions<'a> {
    pub rpms: &'a Path,
    pub output: &'a Path,
    pub signing_key: Option<&'a SigningKey>,
    pub timestamp: u32,
}

#[derive(Debug, Clone)]
pub struct RepoReport {
    pub packages: usize,
    pub repomd: PathBuf,
    pub signature: Option<PathBuf>,
}

pub fn generate(options: RepoOptions<'_>) -> Result<RepoReport, Error> {
    let mut rpms = std::fs::read_dir(options.rpms)
        .map_err(|error| {
            Error::new(
                codes::REPO_001,
                format!("cannot read {}: {error}", options.rpms.display()),
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            Error::new(
                codes::REPO_001,
                format!("cannot read {}: {error}", options.rpms.display()),
            )
        })?;
    rpms.retain(|entry| {
        entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "rpm")
    });
    rpms.sort_by_key(|entry| entry.file_name());
    if rpms.is_empty() {
        return Err(Error::new(
            codes::REPO_002,
            format!("no RPM files in {}", options.rpms.display()),
        ));
    }

    std::fs::create_dir_all(options.output).map_err(|error| {
        Error::new(
            codes::REPO_001,
            format!("cannot create {}: {error}", options.output.display()),
        )
    })?;
    let repodata = options.output.join("repodata");
    std::fs::create_dir_all(&repodata).map_err(|error| {
        Error::new(
            codes::REPO_001,
            format!("cannot create {}: {error}", repodata.display()),
        )
    })?;

    let mut primary = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<metadata xmlns=\"http://linux.duke.edu/metadata/common\" xmlns:rpm=\"http://linux.duke.edu/metadata/rpm\" packages=\"PACKAGE_COUNT\">\n",
    );
    let mut filelists = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<filelists xmlns=\"http://linux.duke.edu/metadata/filelists\" packages=\"PACKAGE_COUNT\">\n",
    );
    let mut other = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<otherdata xmlns=\"http://linux.duke.edu/metadata/other\" packages=\"PACKAGE_COUNT\">\n",
    );

    let mut package_count = 0usize;
    for entry in rpms {
        let source = entry.path();
        let filename = entry.file_name().to_string_lossy().into_owned();
        let target = options.output.join(&filename);
        if source != target && std::fs::hard_link(&source, &target).is_err() {
            std::fs::copy(&source, &target).map_err(|error| {
                Error::new(
                    codes::REPO_001,
                    format!(
                        "cannot copy {} to {}: {error}",
                        source.display(),
                        target.display()
                    ),
                )
            })?;
        }
        let bytes = std::fs::read(&target).map_err(|error| {
            Error::new(
                codes::REPO_001,
                format!("cannot read {}: {error}", target.display()),
            )
        })?;
        let checksum = hex(&Sha256::digest(&bytes));
        let package = rpm::Package::open(&target).map_err(|error| {
            Error::new(
                codes::REPO_003,
                format!(
                    "cannot read RPM metadata from {}: {error}",
                    target.display()
                ),
            )
        })?;
        record(
            &package,
            &filename,
            &checksum,
            bytes.len() as u64,
            &mut primary,
            &mut filelists,
            &mut other,
        )?;
        package_count += 1;
    }
    primary.push_str("</metadata>\n");
    filelists.push_str("</filelists>\n");
    other.push_str("</otherdata>\n");
    primary = primary.replace("PACKAGE_COUNT", &package_count.to_string());
    filelists = filelists.replace("PACKAGE_COUNT", &package_count.to_string());
    other = other.replace("PACKAGE_COUNT", &package_count.to_string());

    let (primary_gz, primary_open) = gzip(primary.as_bytes())?;
    let (filelists_gz, filelists_open) = gzip(filelists.as_bytes())?;
    let (other_gz, other_open) = gzip(other.as_bytes())?;
    write(&repodata.join("primary.xml.gz"), &primary_gz)?;
    write(&repodata.join("filelists.xml.gz"), &filelists_gz)?;
    write(&repodata.join("other.xml.gz"), &other_gz)?;

    let mut repomd = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<repomd xmlns=\"http://linux.duke.edu/metadata/repo\" xmlns:rpm=\"http://linux.duke.edu/metadata/rpm\">\n",
    );
    repomd.push_str(&format!("  <revision>{}</revision>\n", options.timestamp));
    for (kind, file, data, open) in [
        ("primary", "primary.xml.gz", &primary_gz, &primary_open),
        (
            "filelists",
            "filelists.xml.gz",
            &filelists_gz,
            &filelists_open,
        ),
        ("other", "other.xml.gz", &other_gz, &other_open),
    ] {
        repomd.push_str(&format!(
            "  <data type=\"{kind}\">\n    <checksum type=\"sha256\">{}</checksum>\n    <open-checksum type=\"sha256\">{}</open-checksum>\n    <location href=\"repodata/{file}\"/>\n    <timestamp>{}</timestamp>\n    <size>{}</size>\n    <open-size>{}</open-size>\n  </data>\n",
            hex(&Sha256::digest(data)),
            hex(&Sha256::digest(open)),
            options.timestamp,
            data.len(),
            open.len()
        ));
    }
    repomd.push_str("</repomd>\n");
    let repomd_path = repodata.join("repomd.xml");
    write(&repomd_path, repomd.as_bytes())?;

    let signature = match options.signing_key {
        Some(key) => {
            let armored = key.sign_detached(repomd.as_bytes(), Some(options.timestamp))?;
            let path = repodata.join("repomd.xml.asc");
            write(&path, armored.as_bytes())?;
            Some(path)
        }
        None => None,
    };

    Ok(RepoReport {
        packages: package_count,
        repomd: repomd_path,
        signature,
    })
}

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

fn record(
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

fn gzip(data: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Error> {
    use std::io::Write;
    let mut encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).map_err(|error| {
        Error::new(
            codes::REPO_004,
            format!("cannot compress metadata: {error}"),
        )
    })?;
    let compressed = encoder.finish().map_err(|error| {
        Error::new(
            codes::REPO_004,
            format!("cannot compress metadata: {error}"),
        )
    })?;
    Ok((compressed, data.to_vec()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    std::fs::write(path, bytes).map_err(|error| {
        Error::new(
            codes::REPO_001,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
