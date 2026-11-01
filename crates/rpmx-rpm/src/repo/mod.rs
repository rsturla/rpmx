//! Minimal, deterministic RPM repository metadata.
//!
//! The generated layout is what dnf/libdnf consume from a `file://` or HTTPS
//! repository: `repodata/repomd.xml`, primary, filelists and other metadata.
//! With a signing key the command also writes `repodata/repomd.xml.asc`.

use std::path::{Path, PathBuf};

use rpmx_core::error::codes;
use rpmx_core::Error;
use sha2::{Digest, Sha256};

use crate::sign::SigningKey;

mod record;

use record::RepoMetadata;

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

    let mut metadata = RepoMetadata::new();
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
        metadata.add(&package, &filename, &checksum, bytes.len() as u64)?;
    }
    let blobs = metadata.finish();
    let package_count = blobs.packages;
    let primary = blobs.primary;
    let filelists = blobs.filelists;
    let other = blobs.other;

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
