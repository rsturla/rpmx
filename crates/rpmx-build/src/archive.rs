//! Source archive format detection and timestamp inspection.
//!
//! Source archives are the only reproducible record of an upstream release
//! that survives the CAS. Their newest entry mtime is therefore the default
//! source date when a recipe does not pin one explicitly.

use std::io::Read;
use std::path::Path;

use rpmx_base::error::{codes, Error, Result};
use rpmx_language::model::Extract;

/// Resolve the archive format, either from the recipe or from the file magic.
pub(crate) fn format(path: &Path, extract: Option<&Extract>) -> String {
    match extract.and_then(|extract| extract.format.as_deref()) {
        Some("auto") | None => detect(path),
        Some(format) => format.to_string(),
    }
}

/// Read an archive through the decompressor for its format.
pub(crate) fn reader(path: &Path, format: &str) -> Result<Box<dyn Read>> {
    let file = std::fs::File::open(path).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot open {}: {error}", path.display()),
        )
    })?;
    match format {
        "tar.gz" | "tgz" | "gzip" => Ok(Box::new(flate2::read::GzDecoder::new(file))),
        "tar.xz" | "xz" => Ok(Box::new(liblzma::read::XzDecoder::new(file))),
        "tar.zst" | "zstd" => Ok(Box::new(zstd::stream::read::Decoder::new(file).map_err(
            |error| {
                Error::new(
                    codes::SOURCE_031,
                    format!("cannot read {}: {error}", path.display()),
                )
            },
        )?)),
        "tar" => Ok(Box::new(file)),
        other => Err(Error::new(
            codes::SOURCE_031,
            format!(
                "unsupported extract format '{other}' for {}",
                path.display()
            ),
        )),
    }
}

/// The newest non-zero mtime in a tar archive, if the source can be inspected.
///
/// `None` means the source is not an extractable archive or carries no
/// timestamps. Callers then fall back to the explicit recipe value or `0`.
pub(crate) fn latest_mtime(path: &Path, extract: Option<&Extract>) -> Result<Option<u32>> {
    let Some(extract) = extract else {
        return Ok(None);
    };
    let format = format(path, Some(extract));
    if format == "none" || !format.starts_with("tar") {
        return Ok(None);
    }
    let reader = reader(path, &format)?;
    let mut archive = tar::Archive::new(reader);
    let mut newest = 0u32;
    let Ok(entries) = archive.entries() else {
        return Ok(None);
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return Ok(None);
        };
        let mtime = entry.header().mtime().unwrap_or(0);
        if mtime > 0 {
            newest = newest.max(mtime.min(u64::from(u32::MAX)) as u32);
        }
    }
    Ok((newest > 0).then_some(newest))
}

/// Detect a compressed tar archive from its magic bytes.
pub(crate) fn detect(path: &Path) -> String {
    let mut magic = [0u8; 6];
    let Ok(mut file) = std::fs::File::open(path) else {
        return "tar".to_string();
    };
    let read = file.read(&mut magic).unwrap_or(0);
    let magic = &magic[..read];
    if magic.starts_with(&[0x1f, 0x8b]) {
        "tar.gz".to_string()
    } else if magic.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        "tar.xz".to_string()
    } else if magic.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        "tar.zst".to_string()
    } else {
        "tar".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tar_gz(entries: &[(&str, u64)]) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("source.tar.gz");
        let file = std::fs::File::create(&path).expect("file");
        let encoder = flate2::GzBuilder::new()
            .mtime(0)
            .write(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        for (name, mtime) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mtime(*mtime);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, &b"x"[..])
                .expect("entry");
        }
        tar.into_inner().expect("tar").finish().expect("gzip");
        (directory, path)
    }

    #[test]
    fn latest_mtime_uses_the_newest_tar_entry() {
        let (_directory, path) = tar_gz(&[("old", 100), ("new", 200)]);
        let extract = Extract {
            format: Some("auto".to_string()),
            strip_components: Some(1),
            destination: None,
        };
        assert_eq!(
            latest_mtime(&path, Some(&extract)).expect("timestamp"),
            Some(200)
        );
    }

    #[test]
    fn latest_mtime_ignores_zero_timestamps() {
        let (_directory, path) = tar_gz(&[("source", 0)]);
        let extract = Extract {
            format: Some("auto".to_string()),
            strip_components: Some(1),
            destination: None,
        };
        assert_eq!(
            latest_mtime(&path, Some(&extract)).expect("timestamp"),
            None
        );
    }
}
