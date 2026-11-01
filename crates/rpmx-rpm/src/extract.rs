use std::io::Read;
use std::path::Path;

use rpmx_core::{error::codes, Error};

pub use rpmx_core::srpm::ExtractedBundle as ExtractedSrpm;

pub fn extract_srpm(path: &Path, dest: &Path) -> Result<ExtractedSrpm, Error> {
    let package = rpm::Package::open(path).map_err(|error| {
        Error::new(
            codes::EXTRACT_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let compression = package.payload_compression().map_err(|error| {
        Error::new(
            codes::EXTRACT_002,
            format!("cannot read payload format: {error}"),
        )
    })?;

    let mut payload = Vec::new();
    match compression {
        rpm::CompressionType::Gzip => {
            let mut decoder = flate2::read::GzDecoder::new(package.payload.as_slice());
            decoder.read_to_end(&mut payload).map_err(|error| {
                Error::new(
                    codes::EXTRACT_003,
                    format!("cannot decompress gzip payload: {error}"),
                )
            })?;
        }
        rpm::CompressionType::Zstd => {
            let mut decoder = zstd::stream::read::Decoder::new(package.payload.as_slice())
                .map_err(|error| {
                    Error::new(
                        codes::EXTRACT_003,
                        format!("cannot open zstd payload: {error}"),
                    )
                })?;
            decoder.read_to_end(&mut payload).map_err(|error| {
                Error::new(
                    codes::EXTRACT_003,
                    format!("cannot decompress zstd payload: {error}"),
                )
            })?;
        }
        other => {
            return Err(Error::new(
                codes::EXTRACT_002,
                format!("payload compression '{other}' is not supported"),
            ));
        }
    }

    std::fs::create_dir_all(dest).map_err(|error| {
        Error::new(
            codes::EXTRACT_006,
            format!("cannot create {}: {error}", dest.display()),
        )
    })?;
    let files = extract_cpio(&payload, dest)?;
    rpmx_core::srpm::read_bundle(dest, files)
}

fn extract_cpio(payload: &[u8], dest: &Path) -> Result<Vec<String>, Error> {
    let mut files = Vec::new();
    let mut offset = 0;

    while offset + 110 <= payload.len() {
        let header = &payload[offset..offset + 110];
        if !(header.starts_with(b"070701") || header.starts_with(b"070702")) {
            return Err(Error::new(
                codes::EXTRACT_004,
                "payload is not a newc cpio archive",
            ));
        }
        let name_size = hex(header, 94, 102)?;
        let file_size = hex(header, 54, 62)?;
        let name_start = offset + 110;
        let name_end = name_start + name_size;
        if name_end > payload.len() || name_size == 0 {
            return Err(Error::new(codes::EXTRACT_004, "truncated cpio name"));
        }
        let raw_name = &payload[name_start..name_end - 1];
        let name = String::from_utf8(raw_name.to_vec())
            .map_err(|_| Error::new(codes::EXTRACT_004, "cpio name is not UTF-8"))?;
        if name == "TRAILER!!!" {
            break;
        }

        let data_start = align4(name_end);
        let data_end = data_start
            .checked_add(file_size)
            .ok_or_else(|| Error::new(codes::EXTRACT_004, "cpio file size overflow"))?;
        if data_end > payload.len() {
            return Err(Error::new(codes::EXTRACT_004, "truncated cpio data"));
        }

        let relative = name.trim_start_matches("./").trim_start_matches('/');
        if relative.is_empty() || relative.split('/').any(|part| part == "..") {
            return Err(Error::new(
                codes::EXTRACT_005,
                format!("unsafe path in payload: '{name}'"),
            ));
        }
        let target = dest.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::new(
                    codes::EXTRACT_006,
                    format!("cannot create {}: {error}", parent.display()),
                )
            })?;
        }
        std::fs::write(&target, &payload[data_start..data_end]).map_err(|error| {
            Error::new(
                codes::EXTRACT_006,
                format!("cannot write {}: {error}", target.display()),
            )
        })?;
        files.push(relative.to_string());

        offset = align4(data_end);
    }

    Ok(files)
}

fn hex(header: &[u8], start: usize, end: usize) -> Result<usize, Error> {
    let text = std::str::from_utf8(&header[start..end])
        .map_err(|_| Error::new(codes::EXTRACT_004, "cpio header is not ASCII"))?;
    usize::from_str_radix(text, 16)
        .map_err(|_| Error::new(codes::EXTRACT_004, "cpio header has invalid hex"))
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}
