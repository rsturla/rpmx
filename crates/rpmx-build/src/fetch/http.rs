use std::io::{Read, Write};
use std::path::Path;

use rpmx_base::error::{codes, Error, Result};

pub(crate) fn proxy_get(
    agent: &ureq::Agent,
    proxy: &str,
    hex: &str,
    temp_dir: &Path,
) -> Result<Option<tempfile::TempPath>> {
    let url = format!("{}/v1/blobs/sha512/{hex}", proxy.trim_end_matches('/'));
    match agent.get(&url).call() {
        Ok(mut response) => Ok(Some(download_body(response.body_mut(), temp_dir)?)),
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(error) => Err(Error::new(
            codes::FETCH_003,
            format!("proxy request failed: {error}"),
        )),
    }
}

pub(crate) fn get(agent: &ureq::Agent, url: &str, temp_dir: &Path) -> Result<tempfile::TempPath> {
    let mut response = agent
        .get(url)
        .call()
        .map_err(|error| Error::new(codes::FETCH_003, format!("GET {url} failed: {error}")))?;
    download_body(response.body_mut(), temp_dir)
}

pub(crate) fn download_body(body: &mut ureq::Body, temp_dir: &Path) -> Result<tempfile::TempPath> {
    stage_reader(body.as_reader(), temp_dir, "response")
}

/// Stream a reader into an anonymous temporary file. Downloads and local
/// sources are never materialized in memory, so their size is bounded by disk,
/// not by an arbitrary constant.
pub(crate) fn stage_reader(
    reader: impl Read,
    temp_dir: &Path,
    what: &str,
) -> Result<tempfile::TempPath> {
    let mut file = tempfile::NamedTempFile::new_in(temp_dir).map_err(|error| {
        Error::new(
            codes::FETCH_003,
            format!("cannot create a staging file for {what}: {error}"),
        )
    })?;
    let mut reader = rpmx_base::io::BoundedReader::new(reader, rpmx_base::MAX_ARCHIVE_BYTES);
    std::io::copy(&mut reader, &mut file)
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot read {what}: {error}")))?;
    file.flush()
        .map_err(|error| Error::new(codes::FETCH_003, format!("cannot stage {what}: {error}")))?;
    Ok(file.into_temp_path())
}
