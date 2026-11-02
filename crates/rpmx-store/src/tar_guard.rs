use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use rpmx_base::error::{Error, Result};

/// GNU long-name/link and PAX extension headers are read fully into memory by
/// the tar crate; keep that allocation small.
const MAX_EXTENSION_BYTES: u64 = 1024 * 1024;

fn read_tar_header<R: Read>(reader: &mut R, header: &mut [u8; 512]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < header.len() {
        let read = reader.read(&mut header[filled..])?;
        if read == 0 {
            if filled == 0 {
                return Ok(false);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated tar header",
            ));
        }
        filled += read;
    }
    Ok(true)
}

fn parse_tar_size(field: &[u8], subject: &str, code: rpmx_base::error::Code) -> Result<u64> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        let mut value = u64::from(field[0] & 0x7f);
        for byte in &field[1..] {
            value = value
                .checked_shl(8)
                .and_then(|value| value.checked_add(u64::from(*byte)))
                .ok_or_else(|| {
                    Error::new(code, format!("{subject} has an overflowing entry size"))
                })?;
        }
        return Ok(value);
    }
    let text = std::str::from_utf8(field).map_err(|error| {
        Error::new(
            code,
            format!("{subject} has a non-UTF-8 entry size: {error}"),
        )
    })?;
    let text = text.trim_matches(|character: char| character == '\0' || character == ' ');
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(text, 8).map_err(|error| {
        Error::new(
            code,
            format!("{subject} has an invalid entry size: {error}"),
        )
    })
}

/// Parse the one PAX record this transport cares about: the first `size`
/// override tar-rs applies to the next non-extension entry. The record syntax
/// and the "first match wins" rule mirror tar-rs; malformed records fail
/// closed. Sparse PAX records are rejected because neither the guest transfer
/// nor a source snapshot needs them.
fn parse_pax_size(data: &[u8], subject: &str, code: rpmx_base::error::Code) -> Result<Option<u64>> {
    let mut size = None;
    for line in data.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            break;
        }
        let space = line
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(|| Error::new(code, format!("{subject} has a malformed PAX record")))?;
        std::str::from_utf8(&line[..space])
            .ok()
            .and_then(|text| text.parse::<usize>().ok())
            .filter(|length| line.len() + 1 == *length)
            .ok_or_else(|| {
                Error::new(code, format!("{subject} has a malformed PAX record length"))
            })?;
        let key_start = space + 1;
        let equals = line[key_start..]
            .iter()
            .position(|byte| *byte == b'=')
            .ok_or_else(|| Error::new(code, format!("{subject} has a malformed PAX record")))?;
        let key = &line[key_start..key_start + equals];
        let value = &line[key_start + equals + 1..];
        if key.starts_with(b"GNU.sparse.") {
            return Err(Error::new(
                code,
                format!("{subject} contains a sparse entry, which is not supported"),
            ));
        }
        if key == b"size" && size.is_none() {
            size = Some(
                std::str::from_utf8(value)
                    .ok()
                    .and_then(|text| text.parse().ok())
                    .ok_or_else(|| {
                        Error::new(code, format!("{subject} has an invalid PAX size"))
                    })?,
            );
        }
    }
    Ok(size)
}

/// tar-rs only treats a header as an extension when its magic is ustar or GNU
/// oldgnu. A matching typeflag with v7 magic is an ordinary entry.
fn is_recognized_header(header: &[u8; 512]) -> bool {
    (&header[257..263] == b"ustar\0" && &header[263..265] == b"00")
        || (&header[257..263] == b"ustar " && &header[263..265] == b" \0")
}

fn round_up_512(value: u64) -> u64 {
    value.div_ceil(512).saturating_mul(512)
}

fn skip<R: Seek>(
    archive: &mut R,
    bytes: u64,
    subject: &str,
    code: rpmx_base::error::Code,
) -> Result<()> {
    let skip = i64::try_from(bytes)
        .map_err(|_| Error::new(code, format!("{subject} has an entry too large to skip")))?;
    archive
        .seek(SeekFrom::Current(skip))
        .map_err(|error| Error::new(code, format!("cannot scan {subject}: {error}")))?;
    Ok(())
}

fn check_entry_size(
    size: u64,
    max_entry: Option<u64>,
    subject: &str,
    code: rpmx_base::error::Code,
) -> Result<()> {
    if let Some(limit) = max_entry {
        if size > limit {
            return Err(Error::new(
                code,
                format!("{subject} declares a {size}-byte entry; the guest transfer limit is {limit} bytes"),
            ));
        }
    }
    Ok(())
}

/// Reject an archive whose walk would make tar-rs allocate or write more than
/// the configured caps. The walk mirrors tar-rs 0.4.46 for the rules that
/// affect block advancement: magic-gated extension headers, typeflag-only PAX
/// size overrides, and first-`size`-record wins.
pub fn validate<R: Read + Seek>(
    archive: &mut R,
    subject: &str,
    code: rpmx_base::error::Code,
    max_entry: Option<u64>,
) -> Result<()> {
    archive
        .seek(SeekFrom::Start(0))
        .map_err(|error| Error::new(code, format!("cannot rewind {subject}: {error}")))?;
    let mut header = [0u8; 512];
    let mut pending_size = None;
    loop {
        if !read_tar_header(archive, &mut header)
            .map_err(|error| Error::new(code, format!("cannot read {subject}: {error}")))?
        {
            break;
        }
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let header_size = parse_tar_size(&header[124..136], subject, code)?;
        let typeflag = header[156];
        if !is_recognized_header(&header) {
            // tar-rs treats a v7/unknown header as an ordinary entry. Its PAX
            // size override is gated on the typeflag alone: extension typeflags
            // consume the metadata without applying the size, any other type
            // applies it.
            let size = if matches!(typeflag, b'x' | b'L' | b'K' | b'g') {
                pending_size = None;
                header_size
            } else {
                pending_size.take().unwrap_or(header_size)
            };
            check_entry_size(size, max_entry, subject, code)?;
            skip(archive, round_up_512(size), subject, code)?;
            continue;
        }
        match typeflag {
            b'x' => {
                if pending_size.is_some() {
                    return Err(Error::new(
                        code,
                        format!("{subject} has two PAX headers for one entry"),
                    ));
                }
                if header_size > MAX_EXTENSION_BYTES {
                    return Err(Error::new(
                        code,
                        format!(
                            "{subject} has a {header_size}-byte tar extension header; the limit is {MAX_EXTENSION_BYTES} bytes"
                        ),
                    ));
                }
                let mut data = vec![0u8; header_size as usize];
                archive.read_exact(&mut data).map_err(|error| {
                    Error::new(
                        code,
                        format!("cannot read the PAX header of {subject}: {error}"),
                    )
                })?;
                pending_size = parse_pax_size(&data, subject, code)?;
                skip(
                    archive,
                    round_up_512(header_size).saturating_sub(header_size),
                    subject,
                    code,
                )?;
            }
            b'L' | b'K' => {
                if header_size > MAX_EXTENSION_BYTES {
                    return Err(Error::new(
                        code,
                        format!(
                            "{subject} has a {header_size}-byte tar extension header; the limit is {MAX_EXTENSION_BYTES} bytes"
                        ),
                    ));
                }
                skip(archive, round_up_512(header_size), subject, code)?;
            }
            b'g' => {
                if header_size > MAX_EXTENSION_BYTES {
                    return Err(Error::new(
                        code,
                        format!(
                            "{subject} has a {header_size}-byte tar extension header; the limit is {MAX_EXTENSION_BYTES} bytes"
                        ),
                    ));
                }
                // tar-rs attaches a pending PAX override to the global entry
                // as metadata and consumes it there; do the same.
                pending_size = None;
                skip(archive, round_up_512(header_size), subject, code)?;
            }
            b'S' => {
                return Err(Error::new(
                    code,
                    format!("{subject} contains a sparse entry, which is not supported"),
                ));
            }
            _ => {
                let size = pending_size.take().unwrap_or(header_size);
                check_entry_size(size, max_entry, subject, code)?;
                skip(archive, round_up_512(size), subject, code)?;
            }
        }
    }
    archive
        .seek(SeekFrom::Start(0))
        .map_err(|error| Error::new(code, format!("cannot rewind {subject}: {error}")))?;
    Ok(())
}

/// Validate a staged tar and unpack it into `destination`. The archive is
/// rewound and scanned before the tar crate sees it.
pub fn unpack_staged(
    archive: std::fs::File,
    destination: &std::path::Path,
    subject: &str,
    code: rpmx_base::error::Code,
    max_entry: Option<u64>,
) -> Result<()> {
    unpack_staged_with(archive, destination, subject, code, max_entry, 0, ".")
}

/// Validate, strip leading path components, and unpack under `subdir`.
/// A relative link is safe when a purely lexical normalisation of
/// `parent/link` cannot climb above the extraction root.
fn relative_link_stays_inside(parent: &Path, link: &Path) -> bool {
    if link.is_absolute() {
        return false;
    }
    let mut normalized = std::path::PathBuf::new();
    for component in parent.join(link).components() {
        match component {
            std::path::Component::Normal(value) => normalized.push(value),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return false;
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return false,
        }
    }
    true
}

pub fn unpack_staged_with(
    mut archive: std::fs::File,
    destination: &std::path::Path,
    subject: &str,
    code: rpmx_base::error::Code,
    max_entry: Option<u64>,
    strip_components: u32,
    subdir: &str,
) -> Result<()> {
    validate(&mut archive, subject, code, max_entry)?;
    archive
        .rewind()
        .map_err(|error| Error::new(code, format!("cannot rewind {subject}: {error}")))?;
    let base = destination.join(subdir);
    std::fs::create_dir_all(&base)
        .map_err(|error| Error::new(code, format!("cannot create {}: {error}", base.display())))?;
    let mut tar = tar::Archive::new(archive);
    let entries = tar
        .entries()
        .map_err(|error| Error::new(code, format!("cannot read {subject}: {error}")))?;
    for entry in entries {
        let mut entry =
            entry.map_err(|error| Error::new(code, format!("cannot read {subject}: {error}")))?;
        let path = entry
            .path()
            .map_err(|error| Error::new(code, format!("cannot read {subject}: {error}")))?
            .into_owned();
        let components: Vec<_> = path
            .components()
            .filter(|component| !matches!(component, std::path::Component::CurDir))
            .collect();
        let mut stripped = std::path::PathBuf::new();
        for (index, component) in components.iter().enumerate() {
            if (index as u32) < strip_components {
                continue;
            }
            if !matches!(component, std::path::Component::Normal(_)) {
                return Err(Error::new(
                    code,
                    format!("refusing to unpack {subject} entry '{}'", path.display()),
                ));
            }
            stripped.push(component.as_os_str());
        }
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let mut hard_link: Option<std::path::PathBuf> = None;
        match entry.header().entry_type() {
            tar::EntryType::Link => {
                if let Some(link) = entry.link_name().map_err(|error| {
                    Error::new(code, format!("cannot read link in {subject}: {error}"))
                })? {
                    if !relative_link_stays_inside(Path::new(""), &link) {
                        return Err(Error::new(
                            code,
                            format!(
                                "refusing hard link '{}' -> '{}' in {subject}",
                                path.display(),
                                link.display()
                            ),
                        ));
                    }
                    ensure_safe_parents(&base, &link, code, subject)?;
                    hard_link = Some(base.join(&link));
                }
            }
            tar::EntryType::Symlink => {
                if let Some(link) = entry.link_name().map_err(|error| {
                    Error::new(code, format!("cannot read link in {subject}: {error}"))
                })? {
                    let parent = stripped.parent().unwrap_or_else(|| Path::new(""));
                    if !relative_link_stays_inside(parent, &link) {
                        return Err(Error::new(
                            code,
                            format!(
                                "refusing symlink '{}' -> '{}' in {subject}",
                                path.display(),
                                link.display()
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
        let target = base.join(&stripped);
        ensure_safe_parents(&base, &stripped, code, subject)?;
        // Replacing a symlink with a regular file is fine; writing through one
        // is not. `ensure_safe_parents` covers ancestors, this covers the leaf.
        if std::fs::symlink_metadata(&target)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            std::fs::remove_file(&target).map_err(|error| {
                Error::new(
                    code,
                    format!("cannot replace {}: {error}", target.display()),
                )
            })?;
        }
        if let Some(source) = hard_link {
            // The tar crate resolves a hard-link target against the process
            // CWD, not the extraction root. Resolve it here and refuse a
            // symlink or a path outside the tree.
            let metadata = std::fs::symlink_metadata(&source).map_err(|error| {
                Error::new(
                    code,
                    format!(
                        "cannot resolve hard link source {} in {subject}: {error}",
                        source.display()
                    ),
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(Error::new(
                    code,
                    format!(
                        "refusing hard link from non-regular file {} in {subject}",
                        source.display()
                    ),
                ));
            }
            std::fs::hard_link(&source, &target).map_err(|error| {
                Error::new(
                    code,
                    format!(
                        "cannot link {} to {} in {subject}: {error}",
                        source.display(),
                        target.display()
                    ),
                )
            })?;
        } else {
            entry.unpack(&target).map_err(|error| {
                Error::new(
                    code,
                    format!("cannot unpack {} to {}: {error}", subject, target.display()),
                )
            })?;
        }
    }
    Ok(())
}

/// Create the leaf's parent directories, refusing to traverse a symlink.
fn ensure_safe_parents(
    base: &std::path::Path,
    relative: &std::path::Path,
    code: rpmx_base::error::Code,
    subject: &str,
) -> Result<()> {
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut current = base.to_path_buf();
    for component in parent.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::new(
                    code,
                    format!(
                        "refusing to unpack {subject} through symlink {}",
                        current.display()
                    ),
                ))
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(Error::new(
                    code,
                    format!("{} is not a directory", current.display()),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current).map_err(|error| {
                    Error::new(
                        code,
                        format!("cannot create {}: {error}", current.display()),
                    )
                })?;
            }
            Err(error) => {
                return Err(Error::new(
                    code,
                    format!("cannot inspect {}: {error}", current.display()),
                ))
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;
    use rpmx_base::error::codes;
    use std::io::{Seek, SeekFrom, Write};

    fn archive_file(root: &std::path::Path) -> std::fs::File {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(root.join("archive"))
            .expect("archive")
    }

    fn tar_header(typeflag: u8, size: u64) -> [u8; 512] {
        let mut header = [0u8; 512];
        header[156] = typeflag;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let size = format!("{:011o}\0", size);
        header[124..136].copy_from_slice(size.as_bytes());
        header
    }

    fn write_padded(archive: &mut std::fs::File, data: &[u8]) {
        archive.write_all(data).expect("payload");
        let padding = (512 - data.len() % 512) % 512;
        archive.write_all(&vec![0u8; padding]).expect("padding");
    }

    fn pax_record(key: &str, value: &str) -> Vec<u8> {
        let body = format!("{key}={value}\n");
        let mut length = body.len() + 1;
        loop {
            let record = format!("{length} {body}");
            if record.len() == length {
                return record.into_bytes();
            }
            length = record.len();
        }
    }

    fn check(archive: &mut std::fs::File) -> rpmx_base::error::Result<()> {
        validate(archive, "archive from remote", codes::RUNNER_003, None)
    }

    #[test]
    fn oversized_tar_extension_headers_are_rejected() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        archive
            .write_all(&tar_header(b'L', 2 * 1024 * 1024))
            .expect("header");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        let error = check(&mut archive).expect_err("extension cap");
        assert!(
            error.message.contains("extension header"),
            "{}",
            error.message
        );
    }

    #[test]
    fn pax_size_overrides_are_honored_by_the_scanner() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        let pax = pax_record("size", "1024");
        archive
            .write_all(&tar_header(b'x', pax.len() as u64))
            .expect("pax header");
        write_padded(&mut archive, &pax);
        archive
            .write_all(&tar_header(b'0', 0))
            .expect("file header");
        archive.write_all(&[0u8; 512]).expect("zero block");
        archive.write_all(&[b'z'; 512]).expect("data block");
        archive
            .write_all(&tar_header(b'L', 2 * 1024 * 1024))
            .expect("long name header");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        let error = check(&mut archive).expect_err("pax desync");
        assert!(
            error.message.contains("extension header"),
            "{}",
            error.message
        );
    }

    #[test]
    fn duplicate_pax_size_records_use_the_first() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        let mut pax = pax_record("size", "1024");
        pax.extend(pax_record("size", "0"));
        archive
            .write_all(&tar_header(b'x', pax.len() as u64))
            .expect("pax header");
        write_padded(&mut archive, &pax);
        archive
            .write_all(&tar_header(b'0', 0))
            .expect("file header");
        archive.write_all(&[0u8; 512]).expect("zero block");
        archive.write_all(&[b'z'; 512]).expect("data block");
        archive
            .write_all(&tar_header(b'L', 2 * 1024 * 1024))
            .expect("long name header");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        let error = check(&mut archive).expect_err("first size wins");
        assert!(
            error.message.contains("extension header"),
            "{}",
            error.message
        );
    }

    #[test]
    fn malformed_pax_records_are_rejected() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        let pax = b"6 abc\n";
        archive
            .write_all(&tar_header(b'x', pax.len() as u64))
            .expect("pax header");
        write_padded(&mut archive, pax);
        archive
            .write_all(&tar_header(b'0', 0))
            .expect("file header");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        let error = check(&mut archive).expect_err("malformed pax");
        assert!(error.message.contains("malformed PAX"), "{}", error.message);
    }

    #[test]
    fn v7_magic_headers_are_not_extensions() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        let mut header = tar_header(b'L', 1024);
        header[257..265].fill(0);
        archive.write_all(&header).expect("v7 header");
        archive.write_all(&[0u8; 1024]).expect("data");
        archive.write_all(&[0u8; 512]).expect("end marker");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        check(&mut archive).expect("v7 header is a regular entry");
    }

    #[test]
    fn pax_size_applies_to_a_v7_magic_regular_entry() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        let pax = pax_record("size", "1024");
        archive
            .write_all(&tar_header(b'x', pax.len() as u64))
            .expect("pax header");
        write_padded(&mut archive, &pax);
        let mut entry = tar_header(b'0', 0);
        entry[257..265].fill(0);
        archive.write_all(&entry).expect("v7 file header");
        archive.write_all(&[0u8; 512]).expect("zero block");
        archive.write_all(&[b'z'; 512]).expect("data block");
        archive
            .write_all(&tar_header(b'L', 2 * 1024 * 1024))
            .expect("long name header");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        let error = check(&mut archive).expect_err("pax size applies");
        assert!(
            error.message.contains("extension header"),
            "{}",
            error.message
        );
    }

    #[test]
    fn global_header_consumes_a_pending_pax_size() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        let pax = pax_record("size", "1024");
        archive
            .write_all(&tar_header(b'x', pax.len() as u64))
            .expect("pax header");
        write_padded(&mut archive, &pax);
        archive
            .write_all(&tar_header(b'g', 512))
            .expect("global header");
        archive.write_all(&[0u8; 512]).expect("global data");
        archive
            .write_all(&tar_header(b'0', 0))
            .expect("member header");
        archive
            .write_all(&tar_header(b'L', 2 * 1024 * 1024))
            .expect("long name header");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        let error = validate(
            &mut archive,
            "archive from remote",
            codes::RUNNER_003,
            Some(8 * 1024 * 1024 * 1024),
        )
        .expect_err("pending pax must not survive a global header");
        assert!(
            error.message.contains("extension header"),
            "{}",
            error.message
        );
    }

    #[test]
    fn guest_entry_size_cap_is_enforced() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        archive.write_all(&tar_header(b'0', 10)).expect("header");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        let error = validate(
            &mut archive,
            "archive from remote",
            codes::RUNNER_003,
            Some(4),
        )
        .expect_err("entry cap");
        assert!(
            error.message.contains("guest transfer limit"),
            "{}",
            error.message
        );
    }

    #[test]
    fn global_pax_headers_are_skipped() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        archive
            .write_all(&tar_header(b'g', 0))
            .expect("global pax header");
        archive.write_all(&[0u8; 512]).expect("end marker");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        check(&mut archive).expect("global PAX header");
    }

    #[test]
    fn sparse_entries_are_rejected() {
        let root = tempfile::tempdir().expect("work");
        let mut archive = archive_file(root.path());
        archive
            .write_all(&tar_header(b'S', 1024))
            .expect("sparse header");
        archive.seek(SeekFrom::Start(0)).expect("rewind");

        let error = check(&mut archive).expect_err("sparse entry");
        assert!(error.message.contains("sparse entry"), "{}", error.message);
    }
}

#[cfg(test)]
mod extraction_safety {
    use super::{relative_link_stays_inside, unpack_staged_with};
    use rpmx_base::error::codes;
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;

    fn tar_with(build: impl FnOnce(&mut tar::Builder<&mut Vec<u8>>)) -> std::fs::File {
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            build(&mut builder);
            builder.finish().expect("finish");
        }
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("archive.tar");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .expect("create");
        file.write_all(&bytes).expect("write");
        drop(directory);
        file
    }

    fn header(kind: tar::EntryType, path: &str, size: u64) -> tar::Header {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_path(path).expect("path");
        header.set_size(size);
        header.set_mode(0o644);
        header.set_cksum();
        header
    }

    #[test]
    fn parent_components_are_rejected() {
        // `tar::Builder` refuses `..`; a hostile archive does not use it.
        let mut bytes = vec![0u8; 1024];
        let name = b"sub/../../escape";
        bytes[..name.len()].copy_from_slice(name);
        bytes[100..108].copy_from_slice(b"0000644\0");
        bytes[108..116].copy_from_slice(b"0000000\0");
        bytes[116..124].copy_from_slice(b"0000000\0");
        bytes[124..136].copy_from_slice(b"00000000001\0");
        bytes[136..148].copy_from_slice(b"00000000000\0");
        bytes[148..156].fill(b' ');
        bytes[156] = b'0';
        bytes[257..263].copy_from_slice(b"ustar\0");
        bytes[263..265].copy_from_slice(b"00");
        let sum: u32 = bytes[..512].iter().map(|byte| u32::from(*byte)).sum();
        bytes[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("archive.tar");
        std::fs::write(&path, &bytes).expect("archive");
        let archive = std::fs::File::open(&path).expect("open");
        let error = unpack_staged_with(
            archive,
            directory.path(),
            "test",
            codes::SOURCE_031,
            None,
            0,
            ".",
        )
        .expect_err("parent traversal");
        assert_eq!(error.code, codes::SOURCE_031);
        assert!(!directory.path().join("escape").exists());
    }

    #[test]
    fn hard_links_are_rejected() {
        let archive = tar_with(|builder| {
            let mut header = header(tar::EntryType::Link, "link", 0);
            header.set_link_name("/etc/passwd").expect("link");
            header.set_cksum();
            builder.append(&header, std::io::empty()).expect("link");
        });
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(unpack_staged_with(
            archive,
            directory.path(),
            "test",
            codes::SOURCE_031,
            None,
            0,
            ".",
        )
        .is_err());
    }

    #[test]
    fn intra_tree_links_are_within_the_extraction_root() {
        use std::path::Path;
        assert!(relative_link_stays_inside(
            Path::new("usr/bin"),
            Path::new("../libexec/tool")
        ));
        assert!(relative_link_stays_inside(
            Path::new("usr/bin"),
            Path::new("../../etc/config")
        ));
        assert!(!relative_link_stays_inside(
            Path::new("usr/bin"),
            Path::new("../../../outside")
        ));
        assert!(!relative_link_stays_inside(
            Path::new("usr/bin"),
            Path::new("/etc/passwd")
        ));
    }

    #[test]
    fn an_intra_tree_symlink_is_unpacked() {
        let archive = tar_with(|builder| {
            let mut header = header(tar::EntryType::Symlink, "usr/bin/tool", 0);
            header.set_link_name("../libexec/tool").expect("link");
            header.set_cksum();
            builder.append(&header, std::io::empty()).expect("link");
        });
        let directory = tempfile::tempdir().expect("tempdir");
        unpack_staged_with(
            archive,
            directory.path(),
            "test",
            codes::SOURCE_031,
            None,
            0,
            ".",
        )
        .expect("intra-tree symlink");
        let link = directory.path().join("usr/bin/tool");
        assert_eq!(
            std::fs::read_link(link).expect("link"),
            std::path::Path::new("../libexec/tool")
        );
    }

    #[test]
    fn an_intra_tree_hard_link_is_created_from_the_extraction_root() {
        let archive = tar_with(|builder| {
            let mut file = header(tar::EntryType::Regular, "real", 4);
            file.set_cksum();
            builder.append(&file, &b"data"[..]).expect("file");
            let mut link = header(tar::EntryType::Link, "link", 0);
            link.set_link_name("real").expect("link");
            link.set_cksum();
            builder.append(&link, std::io::empty()).expect("link");
        });
        let directory = tempfile::tempdir().expect("tempdir");
        unpack_staged_with(
            archive,
            directory.path(),
            "test",
            codes::SOURCE_031,
            None,
            0,
            ".",
        )
        .expect("intra-tree hard link");
        let real = directory.path().join("real");
        let link = directory.path().join("link");
        assert_eq!(
            std::fs::metadata(&real).expect("real").ino(),
            std::fs::metadata(&link).expect("link").ino()
        );
    }

    #[test]
    fn a_hard_link_to_a_missing_extraction_member_is_rejected() {
        let archive = tar_with(|builder| {
            let mut link = header(tar::EntryType::Link, "leak", 0);
            link.set_link_name("secret.txt").expect("link");
            link.set_cksum();
            builder.append(&link, std::io::empty()).expect("link");
        });
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(unpack_staged_with(
            archive,
            directory.path(),
            "test",
            codes::SOURCE_031,
            None,
            0,
            ".",
        )
        .is_err());
    }

    #[test]
    fn escaping_symlinks_are_rejected() {
        let archive = tar_with(|builder| {
            let mut header = header(tar::EntryType::Symlink, "link", 0);
            header.set_link_name("../../outside").expect("link");
            header.set_cksum();
            builder.append(&header, std::io::empty()).expect("link");
        });
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(unpack_staged_with(
            archive,
            directory.path(),
            "test",
            codes::SOURCE_031,
            None,
            0,
            ".",
        )
        .is_err());
    }
}
