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
    std::fs::create_dir_all(dest).map_err(|error| {
        Error::new(
            codes::EXTRACT_006,
            format!("cannot create {}: {error}", dest.display()),
        )
    })?;
    // Canonical once: containment checks compare against it, and a symlinked
    // destination would otherwise make every check vacuously true.
    let dest = std::fs::canonicalize(dest).map_err(|error| {
        Error::new(
            codes::EXTRACT_006,
            format!("cannot resolve {}: {error}", dest.display()),
        )
    })?;

    let mut files = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut offset = 0u64;
    // First path seen for each cpio inode, so hardlink entries can be linked
    // to it instead of materialising a second copy. Paths are stored
    // relative: an archive may still replace an ancestor with a symlink
    // between entries, so a stored absolute path must never be used without
    // re-resolving it under the destination.
    let mut links: std::collections::BTreeMap<usize, std::path::PathBuf> =
        std::collections::BTreeMap::new();
    // Hardlink members whose data-bearing entry has not been seen yet, with
    // the mode to apply if the group turns out to be empty.
    let mut pending: std::collections::BTreeMap<usize, Vec<(std::path::PathBuf, usize)>> =
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
        let padding = (4 - (file_size % 4)) % 4;

        let relative = name.trim_start_matches("./").trim_start_matches('/');
        if relative.is_empty() {
            // The archive root itself ("./").
            skip_data(payload, file_size as u64)?;
            discard(payload, padding)?;
            offset = padded_end;
            continue;
        }
        if relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(Error::new(
                codes::EXTRACT_005,
                format!("unsafe path in payload: '{name}'"),
            ));
        }
        let path = Path::new(relative);
        let Some(file_name) = path.file_name() else {
            return Err(Error::new(
                codes::EXTRACT_005,
                format!("unsafe path in payload: '{name}'"),
            ));
        };
        // Ancestor components are resolved one at a time. A symlink is only
        // followed when it resolves inside the destination, so an archive can
        // create an escaping link but can never write through it.
        let parent = resolve_parent(&dest, path.parent().unwrap_or_else(|| Path::new("")))?;
        let target = parent.join(file_name);

        match mode & 0o170000 {
            0o040000 => {
                skip_data(payload, file_size as u64)?;
                discard(payload, padding)?;
                remove_existing(&target)?;
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
                let link = read_data(payload, file_size)?;
                discard(payload, padding)?;
                let link = String::from_utf8(link)
                    .map_err(|_| Error::new(codes::EXTRACT_005, "symlink target is not UTF-8"))?;
                if link.is_empty() {
                    return Err(Error::new(
                        codes::EXTRACT_005,
                        format!("empty symlink target for '{relative}'"),
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
                    discard(payload, padding)?;
                    match links.get(&inode) {
                        Some(first) => {
                            let first = entry_path(&dest, first)?;
                            remove_existing(&target)?;
                            std::fs::hard_link(&first, &target).map_err(|error| {
                                Error::new(
                                    codes::EXTRACT_006,
                                    format!("cannot hardlink {}: {error}", target.display()),
                                )
                            })?;
                        }
                        None => pending
                            .entry(inode)
                            .or_default()
                            .push((Path::new(relative).to_path_buf(), mode)),
                    }
                    files.push(relative.to_string());
                    offset = padded_end;
                    continue;
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
                    if let Some(waiting) = pending.remove(&inode) {
                        for (relative, _) in waiting {
                            let path = entry_path(&dest, &relative)?;
                            remove_existing(&path)?;
                            std::fs::hard_link(&target, &path).map_err(|error| {
                                Error::new(
                                    codes::EXTRACT_006,
                                    format!("cannot hardlink {}: {error}", path.display()),
                                )
                            })?;
                        }
                    }
                    links.insert(inode, Path::new(relative).to_path_buf());
                }
            }
            _ => {
                // Devices, fifos and sockets carry no data a sysroot can use.
                skip_data(payload, file_size as u64)?;
                discard(payload, padding)?;
            }
        }
        files.push(relative.to_string());
        offset = padded_end;
    }

    // A hardlinked empty file has no data-bearing member: every entry carries
    // size zero. Materialise the group instead of rejecting it.
    for (_, waiting) in pending {
        let Some(((first_relative, mode), rest)) = waiting.split_first() else {
            continue;
        };
        let first = entry_path(&dest, first_relative)?;
        remove_existing(&first)?;
        std::fs::File::create(&first).map_err(|error| {
            Error::new(
                codes::EXTRACT_006,
                format!("cannot create {}: {error}", first.display()),
            )
        })?;
        set_mode(&first, *mode)?;
        for (relative, _) in rest {
            let path = entry_path(&dest, relative)?;
            remove_existing(&path)?;
            std::fs::hard_link(&first, &path).map_err(|error| {
                Error::new(
                    codes::EXTRACT_006,
                    format!("cannot hardlink {}: {error}", path.display()),
                )
            })?;
        }
    }

    Ok(files)
}

/// Resolve a relative parent path under `dest`, refusing to leave it.
///
/// A symlink component is followed only when it resolves inside `dest`; a
/// legitimate usrmerge link such as `lib -> usr/lib` keeps working, and an
/// `escape -> /home/user` link becomes unusable instead of a write primitive.
fn resolve_parent(dest: &Path, relative: &Path) -> Result<std::path::PathBuf> {
    let mut current = dest.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(Error::new(
                codes::EXTRACT_005,
                format!("unsafe path in payload: '{}'", relative.display()),
            ));
        };
        let next = current.join(name);
        match std::fs::symlink_metadata(&next) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let resolved = std::fs::canonicalize(&next).map_err(|error| {
                    Error::new(
                        codes::EXTRACT_005,
                        format!("cannot resolve symlink {}: {error}", next.display()),
                    )
                })?;
                if !resolved.starts_with(dest) {
                    return Err(Error::new(
                        codes::EXTRACT_005,
                        format!(
                            "payload entry '{}' escapes the destination through a symlink",
                            relative.display()
                        ),
                    ));
                }
                current = resolved;
            }
            Ok(metadata) if metadata.is_dir() => current = next,
            Ok(_) => {
                return Err(Error::new(
                    codes::EXTRACT_005,
                    format!("'{next}' is not a directory", next = next.display()),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&next).map_err(|error| {
                    Error::new(
                        codes::EXTRACT_006,
                        format!("cannot create {}: {error}", next.display()),
                    )
                })?;
                current = next;
            }
            Err(error) => {
                return Err(Error::new(
                    codes::EXTRACT_006,
                    format!("cannot inspect {}: {error}", next.display()),
                ));
            }
        }
    }
    Ok(current)
}

/// The confined path for a recorded relative entry.
///
/// Stored paths are never trusted as-is: an archive can replace an ancestor
/// with a symlink after the entry was recorded, so the parent is resolved
/// again against the current filesystem.
fn entry_path(dest: &Path, relative: &Path) -> Result<std::path::PathBuf> {
    let parent = resolve_parent(dest, relative.parent().unwrap_or_else(|| Path::new("")))?;
    let name = relative.file_name().ok_or_else(|| {
        Error::new(
            codes::EXTRACT_005,
            format!("unsafe stored path '{}'", relative.display()),
        )
    })?;
    Ok(parent.join(name))
}

fn skip_data<R: Read>(payload: &mut R, mut bytes: u64) -> Result<()> {
    let mut buffer = [0u8; 8192];
    while bytes > 0 {
        let wanted = bytes.min(buffer.len() as u64) as usize;
        payload.read_exact(&mut buffer[..wanted]).map_err(|error| {
            Error::new(
                codes::EXTRACT_004,
                format!("cannot skip cpio data: {error}"),
            )
        })?;
        bytes -= wanted as u64;
    }
    Ok(())
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

    #[test]
    fn a_symlink_cannot_be_used_to_write_outside_the_destination() {
        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        let victim = outside.join("victim");
        std::fs::write(&victim, b"keep").expect("victim");

        let mut cpio = cpio_entry_with(
            "escape",
            outside.to_str().expect("utf8").as_bytes(),
            outside.to_str().expect("utf8").len(),
            0o120777,
            1,
            1,
        );
        cpio.extend(cpio_entry_with(
            "escape/victim",
            b"pwned",
            5,
            0o100644,
            2,
            1,
        ));
        cpio.extend(cpio_trailer());

        let error = super::extract_decoded(std::io::Cursor::new(cpio), &dest)
            .expect_err("an escaping symlink must not be followed");
        assert_eq!(error.code, "RPMX-EXTRACT-005");
        assert_eq!(std::fs::read(&victim).expect("victim"), b"keep");
    }

    #[test]
    fn a_usrmerge_symlink_inside_the_root_is_followed() {
        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let mut cpio = cpio_entry_with("usr/lib", b"", 0, 0o040755, 1, 2);
        cpio.extend(cpio_entry_with("lib", b"usr/lib", 7, 0o120777, 2, 1));
        cpio.extend(cpio_entry_with("lib/example", b"ok", 2, 0o100644, 3, 1));
        cpio.extend(cpio_trailer());

        super::extract_decoded(std::io::Cursor::new(cpio), &dest).expect("extract");
        assert_eq!(
            std::fs::read(dest.join("usr/lib/example")).expect("file"),
            b"ok"
        );
    }

    #[test]
    fn a_hardlink_group_whose_data_arrives_last_still_shares_it() {
        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let mut cpio = cpio_entry_with("usr/bin/a", b"", 0, 0o100755, 42, 2);
        cpio.extend(cpio_entry_with("usr/bin/b", b"same", 4, 0o100755, 42, 2));
        cpio.extend(cpio_trailer());

        super::extract_decoded(std::io::Cursor::new(cpio), &dest).expect("extract");
        assert_eq!(std::fs::read(dest.join("usr/bin/a")).expect("a"), b"same");
        assert_eq!(std::fs::read(dest.join("usr/bin/b")).expect("b"), b"same");
    }

    #[test]
    fn an_entry_named_dot_is_rejected() {
        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        std::fs::create_dir_all(&dest).expect("dest");
        std::fs::write(dest.join("keep"), b"keep").expect("keep");
        let mut cpio = cpio_entry_with(".", b"x", 1, 0o100644, 1, 1);
        cpio.extend(cpio_trailer());

        let error = super::extract_decoded(std::io::Cursor::new(cpio), &dest)
            .expect_err("a '.' entry must be rejected");
        assert_eq!(error.code, "RPMX-EXTRACT-005");
        assert_eq!(std::fs::read(dest.join("keep")).expect("keep"), b"keep");
    }

    #[test]
    fn a_directory_entry_declaring_data_does_not_panic() {
        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let padding = [0u8; 8];
        let mut cpio = cpio_entry_with("d", &padding, 8, 0o040755, 1, 2);
        cpio.extend(cpio_trailer());
        super::extract_decoded(std::io::Cursor::new(cpio), &dest).expect("extract");
        assert!(dest.join("d").is_dir());
    }

    #[test]
    fn the_archive_root_is_skipped_and_a_directory_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let mut cpio = cpio_entry_with("./", b"", 0, 0o040755, 0, 2);
        cpio.extend(cpio_entry_with("priv", b"", 0, 0o040700, 1, 2));
        cpio.extend(cpio_entry_with("priv/file", b"x", 1, 0o100600, 2, 1));
        cpio.extend(cpio_trailer());

        super::extract_decoded(std::io::Cursor::new(cpio), &dest).expect("extract");
        let mode = std::fs::metadata(dest.join("priv"))
            .expect("directory")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        assert_eq!(std::fs::read(dest.join("priv/file")).expect("file"), b"x");
    }

    #[test]
    fn hardlinked_empty_files_are_materialised() {
        use std::os::unix::fs::MetadataExt;

        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let mut cpio = cpio_entry_with("usr/bin/e1", b"", 0, 0o100644, 7, 2);
        cpio.extend(cpio_entry_with("usr/bin/e2", b"", 0, 0o100644, 7, 2));
        cpio.extend(cpio_trailer());

        super::extract_decoded(std::io::Cursor::new(cpio), &dest).expect("extract");
        let a = dest.join("usr/bin/e1");
        let b = dest.join("usr/bin/e2");
        assert_eq!(std::fs::read(&a).expect("e1"), b"");
        assert_eq!(std::fs::read(&b).expect("e2"), b"");
        assert_eq!(
            std::fs::metadata(&a).expect("a").ino(),
            std::fs::metadata(&b).expect("b").ino()
        );
    }

    #[test]
    fn a_recorded_hardlink_path_is_re_resolved_before_use() {
        let root = tempfile::tempdir().expect("work");
        let dest = root.path().join("dest");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&outside).expect("outside");
        let victim = outside.join("member");
        std::fs::write(&victim, b"original").expect("victim");

        // The member is recorded while `esc` is a real directory, then the
        // archive swaps it for an escaping symlink before the data entry
        // flushes the group.
        let mut cpio = cpio_entry_with("esc/member", b"", 0, 0o100644, 42, 2);
        let link = outside.to_str().expect("utf8");
        cpio.extend(cpio_entry_with(
            "esc",
            link.as_bytes(),
            link.len(),
            0o120777,
            9,
            1,
        ));
        cpio.extend(cpio_entry_with("data", b"PWNED", 5, 0o100644, 42, 2));
        cpio.extend(cpio_trailer());

        let error = super::extract_decoded(std::io::Cursor::new(cpio), &dest)
            .expect_err("a swapped parent must be rejected at use time");
        assert_eq!(error.code, "RPMX-EXTRACT-005");
        assert_eq!(std::fs::read(&victim).expect("victim"), b"original");
    }
}
