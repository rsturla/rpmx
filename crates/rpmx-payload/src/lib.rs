//! Safe extraction of an RPM payload into a directory.
//!
//! The payload is decompressed into an anonymous staging file first, so a
//! corrupt gzip or zstd footer is rejected before anything reaches the
//! destination, and the cpio walk knows the total length for bounds checks.
//! Entries are written to a unique staging name and renamed into place, so a
//! truncated archive never replaces a file that already existed.
//!
//! Both the SRPM reader and build-root assembly use this; neither may execute
//! the payload, so a target-architecture sysroot needs no emulation.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use rpmx_base::error::{codes, Error, Result};

/// CPIO names are short in practice; cap them so a hostile header cannot make
/// the parser allocate arbitrarily.
const MAX_CPIO_NAME: usize = 4096;

/// Extract one RPM payload into `dest`, returning the relative paths written.
pub fn extract_payload(package: &rpm::Package, dest: &Path) -> Result<Vec<String>> {
    let compression = package.payload_compression().map_err(|error| {
        Error::new(
            codes::EXTRACT_002,
            format!("cannot read payload format: {error}"),
        )
    })?;
    match compression {
        rpm::CompressionType::Gzip => extract_decoded(
            flate2::read::GzDecoder::new(package.payload.as_slice()),
            dest,
        ),
        rpm::CompressionType::Zstd => {
            let decoder =
                zstd::stream::read::Decoder::new(package.payload.as_slice()).map_err(|error| {
                    Error::new(
                        codes::EXTRACT_003,
                        format!("cannot open zstd payload: {error}"),
                    )
                })?;
            extract_decoded(decoder, dest)
        }
        other => Err(Error::new(
            codes::EXTRACT_002,
            format!("payload compression '{other}' is not supported"),
        )),
    }
}

fn extract_decoded<R: Read>(reader: R, dest: &Path) -> Result<Vec<String>> {
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
    let mut reader = rpmx_base::io::BoundedReader::new(reader, rpmx_base::MAX_ARCHIVE_BYTES);
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

fn read_exact<R: Read>(payload: &mut R, buffer: &mut [u8], what: &str) -> Result<()> {
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

fn discard<R: Read>(payload: &mut R, bytes: usize) -> Result<()> {
    if bytes == 0 {
        return Ok(());
    }
    let mut padding = [0u8; 4];
    read_exact(payload, &mut padding[..bytes], "padding")
}

fn extract_cpio<R: Read>(payload: &mut R, dest: &Path, total: u64) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut offset = 0u64;
    // First path seen for each cpio inode, so hardlink entries can be linked
    // to it instead of materialising a second copy.
    let mut links: std::collections::BTreeMap<usize, std::path::PathBuf> =
        std::collections::BTreeMap::new();

    loop {
        let mut header = [0u8; 110];
        read_exact(payload, &mut header, "header")?;
        if !(header.starts_with(b"070701") || header.starts_with(b"070702")) {
            return Err(Error::new(
                codes::EXTRACT_004,
                "payload is not a newc cpio archive",
            ));
        }
        let inode = hex(&header, 6, 14)?;
        let mode = hex(&header, 14, 22)?;
        let links_count = hex(&header, 38, 46)?;
        let file_size = hex(&header, 54, 62)?;
        let name_size = hex(&header, 94, 102)?;
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
        if relative.is_empty() {
            // The archive root itself ("./"): nothing to create.
            discard(payload, file_size)?;
            discard(payload, (4 - (file_size % 4)) % 4)?;
            offset = padded_end;
            continue;
        }
        if relative.split('/').any(|part| part == "..") {
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

        let padding = (4 - (file_size % 4)) % 4;
        match mode & 0o170000 {
            0o040000 => {
                // Directory entry: no data, just the mode.
                discard(payload, file_size)?;
                discard(payload, padding)?;
                std::fs::create_dir_all(&target).map_err(|error| {
                    Error::new(
                        codes::EXTRACT_006,
                        format!("cannot create {}: {error}", target.display()),
                    )
                })?;
                // Keep the owner-write bit: a real filesystem package ships
                // usr/bin as 0555, and the next package still has to populate
                // it. A sysroot is mounted read-only anyway.
                set_mode(&target, mode | 0o200)?;
            }
            0o120000 => {
                // Symlink entry: the data is the link target.
                let link = read_data(payload, file_size)?;
                discard(payload, padding)?;
                let link = String::from_utf8(link)
                    .map_err(|_| Error::new(codes::EXTRACT_005, "symlink target is not UTF-8"))?;
                if link.is_empty()
                    || std::path::Path::new(&link).is_absolute() && link.contains("..")
                {
                    return Err(Error::new(
                        codes::EXTRACT_005,
                        format!("unsafe symlink target in payload: '{link}'"),
                    ));
                }
                remove_existing(&target)?;
                std::os::unix::fs::symlink(&link, &target).map_err(|error| {
                    Error::new(
                        codes::EXTRACT_006,
                        format!("cannot create symlink {}: {error}", target.display()),
                    )
                })?;
            }
            0o100000 | 0 => {
                if links_count > 1 && file_size == 0 {
                    if let Some(first) = links.get(&inode) {
                        discard(payload, padding)?;
                        remove_existing(&target)?;
                        std::fs::hard_link(first, &target).map_err(|error| {
                            Error::new(
                                codes::EXTRACT_006,
                                format!("cannot hardlink {}: {error}", target.display()),
                            )
                        })?;
                        files.push(relative.to_string());
                        offset = padded_end;
                        continue;
                    }
                }
                let staged = staged_path(&target);
                if let Err(error) = write_entry(payload, &staged, file_size as u64, &mut chunk) {
                    let _ = std::fs::remove_file(&staged);
                    return Err(error);
                }
                if let Err(error) = discard(payload, padding) {
                    let _ = std::fs::remove_file(&staged);
                    return Err(error);
                }
                remove_existing(&target)?;
                std::fs::rename(&staged, &target).map_err(|error| {
                    let _ = std::fs::remove_file(&staged);
                    Error::new(
                        codes::EXTRACT_006,
                        format!("cannot write {}: {error}", target.display()),
                    )
                })?;
                set_mode(&target, mode)?;
                if links_count > 1 {
                    links.insert(inode, target);
                }
            }
            _ => {
                // Devices, fifos and sockets carry no data a sysroot can use.
                discard(payload, file_size)?;
                discard(payload, padding)?;
            }
        }
        files.push(relative.to_string());
        offset = padded_end;
    }

    Ok(files)
}

fn read_data<R: Read>(payload: &mut R, size: usize) -> Result<Vec<u8>> {
    // A symlink target is a path; anything larger than this is hostile.
    if size > 64 * 1024 {
        return Err(Error::new(
            codes::EXTRACT_005,
            "cpio entry is too large for its type",
        ));
    }
    let mut data = vec![0u8; size];
    payload.read_exact(&mut data).map_err(|error| {
        Error::new(
            codes::EXTRACT_004,
            format!("cannot read cpio data: {error}"),
        )
    })?;
    Ok(data)
}

fn remove_existing(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path).map_err(|error| {
            Error::new(
                codes::EXTRACT_006,
                format!("cannot replace {}: {error}", path.display()),
            )
        }),
        Ok(_) => std::fs::remove_file(path).map_err(|error| {
            Error::new(
                codes::EXTRACT_006,
                format!("cannot replace {}: {error}", path.display()),
            )
        }),
        Err(_) => Ok(()),
    }
}

fn set_mode(path: &Path, mode: usize) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if mode & 0o7777 == 0 {
        return Ok(());
    }
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode((mode & 0o7777) as u32),
    )
    .map_err(|error| {
        Error::new(
            codes::EXTRACT_006,
            format!("cannot set permissions on {}: {error}", path.display()),
        )
    })
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
) -> Result<()> {
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

fn hex(header: &[u8], start: usize, end: usize) -> Result<usize> {
    let text = std::str::from_utf8(&header[start..end])
        .map_err(|_| Error::new(codes::EXTRACT_004, "cpio header is not ASCII"))?;
    usize::from_str_radix(text, 16)
        .map_err(|_| Error::new(codes::EXTRACT_004, "cpio header has invalid hex"))
}

#[cfg(test)]
mod tests {
    fn cpio_entry(name: &str, data: &[u8], declared: usize) -> Vec<u8> {
        cpio_entry_with(name, data, declared, 0o100644, 0, 1)
    }

    fn cpio_entry_with(
        name: &str,
        data: &[u8],
        declared: usize,
        mode: u32,
        ino: u32,
        nlink: u32,
    ) -> Vec<u8> {
        let fields = [
            ino,
            mode,
            0,
            0,
            nlink,
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

    #[test]
    fn a_symlink_entry_is_created_as_a_symlink() {
        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let mut cpio = cpio_entry_with("usr/lib64/libc.so", b"libc.so.6", 9, 0o120777, 1, 1);
        cpio.extend(cpio_trailer());
        super::extract_decoded(std::io::Cursor::new(cpio), &dest).expect("extract");

        let link = dest.join("usr/lib64/libc.so");
        let metadata = std::fs::symlink_metadata(&link).expect("symlink");
        assert!(metadata.file_type().is_symlink(), "must stay a symlink");
        assert_eq!(
            std::fs::read_link(&link).expect("target"),
            std::path::Path::new("libc.so.6")
        );
    }

    #[test]
    fn a_directory_entry_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let mut cpio = cpio_entry_with("usr/lib64", b"", 0, 0o040755, 1, 2);
        cpio.extend(cpio_trailer());
        super::extract_decoded(std::io::Cursor::new(cpio), &dest).expect("extract");

        let metadata = std::fs::metadata(dest.join("usr/lib64")).expect("directory");
        assert!(metadata.is_dir());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o755);
    }

    #[test]
    fn a_hardlink_entry_links_to_the_first_copy() {
        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let mut cpio = cpio_entry_with("usr/bin/a", b"same", 4, 0o100755, 42, 2);
        cpio.extend(cpio_entry_with("usr/bin/b", b"", 0, 0o100755, 42, 2));
        cpio.extend(cpio_trailer());
        super::extract_decoded(std::io::Cursor::new(cpio), &dest).expect("extract");

        let a = dest.join("usr/bin/a");
        let b = dest.join("usr/bin/b");
        assert_eq!(std::fs::read(&a).expect("a"), b"same");
        assert_eq!(std::fs::read(&b).expect("b"), b"same");
        let a_meta = std::fs::metadata(&a).expect("a");
        let b_meta = std::fs::metadata(&b).expect("b");
        use std::os::unix::fs::MetadataExt;
        assert_eq!(a_meta.ino(), b_meta.ino(), "must be one inode");
    }
}
