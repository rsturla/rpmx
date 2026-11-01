use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use rpmx_core::{error::codes, Error};

pub use rpmx_core::srpm::ExtractedBundle as ExtractedSrpm;

/// CPIO names are short in practice; cap them so a hostile header cannot make
/// the parser allocate arbitrarily.
const MAX_CPIO_NAME: usize = 4096;

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

    let files = match compression {
        rpm::CompressionType::Gzip => {
            let decoder = flate2::read::GzDecoder::new(package.payload.as_slice());
            extract_decoded(decoder, dest)?
        }
        rpm::CompressionType::Zstd => {
            let decoder =
                zstd::stream::read::Decoder::new(package.payload.as_slice()).map_err(|error| {
                    Error::new(
                        codes::EXTRACT_003,
                        format!("cannot open zstd payload: {error}"),
                    )
                })?;
            extract_decoded(decoder, dest)?
        }
        other => {
            return Err(Error::new(
                codes::EXTRACT_002,
                format!("payload compression '{other}' is not supported"),
            ));
        }
    };

    rpmx_core::srpm::read_bundle(dest, files)
}

fn extract_decoded<R: Read>(reader: R, dest: &Path) -> Result<Vec<String>, Error> {
    std::fs::create_dir_all(dest).map_err(|error| {
        Error::new(
            codes::EXTRACT_006,
            format!("cannot create {}: {error}", dest.display()),
        )
    })?;
    // Decompress fully into an anonymous file first: this validates the gzip
    // or zstd footer before anything reaches the destination, and gives the
    // cpio walk a known total length for pre-write bounds checks.
    let mut staged = tempfile::tempfile().map_err(|error| {
        Error::new(
            codes::EXTRACT_006,
            format!("cannot create a staging file: {error}"),
        )
    })?;
    let mut reader = rpmx_base::io::BoundedReader::new(reader, rpmx_core::MAX_ARCHIVE_BYTES);
    std::io::copy(&mut reader, &mut staged).map_err(|error| {
        Error::new(
            codes::EXTRACT_003,
            format!("cannot decompress or stage payload: {error}"),
        )
    })?;
    let total = staged
        .metadata()
        .map_err(|error| {
            Error::new(
                codes::EXTRACT_006,
                format!("cannot stat the staging file: {error}"),
            )
        })?
        .len();
    staged.seek(SeekFrom::Start(0)).map_err(|error| {
        Error::new(
            codes::EXTRACT_006,
            format!("cannot rewind the staging file: {error}"),
        )
    })?;
    extract_cpio(&mut staged, dest, total)
}

fn read_exact<R: Read>(payload: &mut R, buffer: &mut [u8], what: &str) -> Result<(), Error> {
    payload.read_exact(buffer).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::new(codes::EXTRACT_004, format!("truncated cpio {what}"))
        } else {
            Error::new(
                codes::EXTRACT_003,
                format!("cannot read cpio {what}: {error}"),
            )
        }
    })
}

fn discard<R: Read>(payload: &mut R, bytes: usize) -> Result<(), Error> {
    if bytes == 0 {
        return Ok(());
    }
    let mut padding = [0u8; 4];
    read_exact(payload, &mut padding[..bytes], "padding")
}

fn extract_cpio<R: Read>(payload: &mut R, dest: &Path, total: u64) -> Result<Vec<String>, Error> {
    let mut files = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut offset = 0u64;

    loop {
        let mut header = [0u8; 110];
        read_exact(payload, &mut header, "header")?;
        if !(header.starts_with(b"070701") || header.starts_with(b"070702")) {
            return Err(Error::new(
                codes::EXTRACT_004,
                "payload is not a newc cpio archive",
            ));
        }
        let name_size = hex(&header, 94, 102)?;
        let file_size = hex(&header, 54, 62)?;
        if name_size == 0 || name_size > MAX_CPIO_NAME {
            return Err(Error::new(
                codes::EXTRACT_004,
                "cpio name is empty or too long",
            ));
        }
        let mut name_bytes = vec![0u8; name_size];
        read_exact(payload, &mut name_bytes, "name")?;
        if name_bytes.last() != Some(&0) {
            return Err(Error::new(
                codes::EXTRACT_004,
                "cpio name is not NUL-terminated",
            ));
        }
        let name = std::str::from_utf8(&name_bytes[..name_size - 1])
            .map_err(|_| Error::new(codes::EXTRACT_004, "cpio name is not UTF-8"))?;
        if name == "TRAILER!!!" {
            break;
        }
        let name_padding = (4 - ((110 + name_size) % 4)) % 4;
        discard(payload, name_padding)?;
        let data_start = offset + 110 + name_size as u64 + name_padding as u64;
        let data_end = data_start
            .checked_add(file_size as u64)
            .ok_or_else(|| Error::new(codes::EXTRACT_004, "cpio file size overflow"))?;
        let padded_end = data_end
            .checked_add(((4 - (file_size % 4)) % 4) as u64)
            .ok_or_else(|| Error::new(codes::EXTRACT_004, "cpio file size overflow"))?;
        if padded_end > total {
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
        let staged = staged_path(&target);
        if let Err(error) = write_entry(payload, &staged, file_size as u64, &mut chunk) {
            let _ = std::fs::remove_file(&staged);
            return Err(error);
        }
        if let Err(error) = discard(payload, (4 - (file_size % 4)) % 4) {
            let _ = std::fs::remove_file(&staged);
            return Err(error);
        }
        std::fs::rename(&staged, &target).map_err(|error| {
            let _ = std::fs::remove_file(&staged);
            Error::new(
                codes::EXTRACT_006,
                format!("cannot write {}: {error}", target.display()),
            )
        })?;
        files.push(relative.to_string());
        offset = padded_end;
    }

    Ok(files)
}

fn staged_path(target: &Path) -> std::path::PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(".rpmx-extract-{:016x}", rand::random::<u64>()))
}

fn write_entry<R: Read>(
    payload: &mut R,
    staged: &Path,
    file_size: u64,
    chunk: &mut [u8],
) -> Result<(), Error> {
    let mut file = std::fs::File::create(staged).map_err(|error| {
        Error::new(
            codes::EXTRACT_006,
            format!("cannot write {}: {error}", staged.display()),
        )
    })?;
    let mut remaining = file_size;
    while remaining > 0 {
        let wanted = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(chunk.len());
        let read = payload.read(&mut chunk[..wanted]).map_err(|error| {
            Error::new(
                codes::EXTRACT_003,
                format!("cannot read cpio data: {error}"),
            )
        })?;
        if read == 0 {
            return Err(Error::new(codes::EXTRACT_004, "truncated cpio data"));
        }
        file.write_all(&chunk[..read]).map_err(|error| {
            Error::new(
                codes::EXTRACT_006,
                format!("cannot write {}: {error}", staged.display()),
            )
        })?;
        remaining -= read as u64;
    }
    Ok(())
}

fn hex(header: &[u8], start: usize, end: usize) -> Result<usize, Error> {
    let text = std::str::from_utf8(&header[start..end])
        .map_err(|_| Error::new(codes::EXTRACT_004, "cpio header is not ASCII"))?;
    usize::from_str_radix(text, 16)
        .map_err(|_| Error::new(codes::EXTRACT_004, "cpio header has invalid hex"))
}

#[cfg(test)]
mod tests {
    fn cpio_entry(name: &str, data: &[u8], declared: usize) -> Vec<u8> {
        let fields = [
            0u32,
            0o100644,
            0,
            0,
            1,
            0,
            declared as u32,
            0,
            0,
            0,
            0,
            (name.len() + 1) as u32,
            0,
        ];
        let mut header = String::from("070701");
        for field in fields {
            header.push_str(&format!("{field:08x}"));
        }
        assert_eq!(header.len(), 110);
        let mut bytes = header.into_bytes();
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(data);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        bytes
    }

    fn cpio_trailer() -> Vec<u8> {
        cpio_entry("TRAILER!!!", b"", 0)
    }

    #[test]
    fn corrupt_gzip_trailer_is_rejected_before_extraction() {
        use std::io::Write;

        let mut cpio = cpio_entry("a", b"x", 1);
        cpio.extend(cpio_trailer());
        let mut gzip = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut gzip, flate2::Compression::default());
            encoder.write_all(&cpio).expect("gzip payload");
            encoder.finish().expect("gzip footer");
        }
        let last = gzip.len() - 1;
        gzip[last] ^= 0xff;

        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let error = super::extract_decoded(
            flate2::read::GzDecoder::new(std::io::Cursor::new(gzip)),
            &dest,
        )
        .expect_err("corrupt gzip");
        assert_eq!(error.code, "RPMX-EXTRACT-003");
        assert!(!dest.join("a").exists(), "corrupt archive must not extract");
    }

    #[test]
    fn truncated_entry_does_not_replace_the_destination() {
        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        std::fs::create_dir_all(&dest).expect("dest");
        std::fs::write(dest.join("a"), b"previous").expect("existing file");
        let cpio = cpio_entry("a", b"abc", 100);

        let error =
            super::extract_decoded(std::io::Cursor::new(cpio), &dest).expect_err("truncated data");
        assert_eq!(error.code, "RPMX-EXTRACT-004");
        assert_eq!(
            std::fs::read(dest.join("a")).expect("existing file"),
            b"previous"
        );
        assert_eq!(
            std::fs::read_dir(&dest).expect("dest").count(),
            1,
            "no staging files may be left behind"
        );
    }
}
