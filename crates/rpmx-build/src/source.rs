use std::io::Write;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

use crate::fetch::{fetch_sources, FetchOptions};
use crate::package;
use rpmx_base::error::{codes, Error, Result};
use rpmx_language::model::Recipe;
use rpmx_language::plan::Plan;
use rpmx_runner::runner::RunnerKind;
use rpmx_runner::sandbox::{Sandbox, SandboxConfig};

pub struct SourceOptions<'a> {
    pub recipe: &'a Recipe,
    pub plan: &'a Plan,
    pub cache: &'a Path,
    pub proxy: Option<&'a str>,
    pub offline: bool,
    pub base_dir: &'a Path,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    pub output: &'a Path,
    pub source_date_epoch: u32,
    pub jobs: NonZeroU32,
    pub step_timeout: NonZeroU64,
    pub root: Option<&'a Path>,
    pub runner: &'a str,
    pub recipe_dir: &'a Path,
}

pub fn assemble(options: SourceOptions<'_>) -> Result<PathBuf> {
    let fetched = fetch_sources(
        options.recipe,
        FetchOptions {
            cache: options.cache,
            proxy: options.proxy,
            offline: options.offline,
            base_dir: options.base_dir,
        },
    )?;

    std::fs::create_dir_all(options.workspace).map_err(|error| {
        Error::new(
            codes::SOURCE_030,
            format!("cannot create {}: {error}", options.workspace.display()),
        )
    })?;
    std::fs::create_dir_all(options.stage).map_err(|error| {
        Error::new(
            codes::SOURCE_030,
            format!("cannot create {}: {error}", options.stage.display()),
        )
    })?;

    for source in &fetched {
        materialize(
            &source.path,
            &source.name,
            source.extract.as_ref(),
            options.workspace,
        )?;
    }

    let host_arch = rpmx_base::arch::Arch::current()?;
    let target_arch = rpmx_base::arch::Arch::parse(&options.plan.arch)?;
    let sandbox = Sandbox::new(SandboxConfig {
        runner: options.runner.parse::<RunnerKind>()?,
        host: host_arch,
        target: target_arch,
        workspace: options.workspace,
        stage: options.stage,
        root: options.root,
        image: None,
        source_date_epoch: options.source_date_epoch,
        jobs: options.jobs,
        timeout: options.step_timeout,
        debuginfo: false,
    })?;

    apply_patches(
        &sandbox,
        options.recipe_dir,
        &options.recipe.patches,
        options.workspace,
    )?;

    if let Some(source_assembly) = &options.plan.source_assembly {
        for step in &source_assembly.steps {
            sandbox.run(step)?;
        }
    }

    sandbox.sync()?;

    std::fs::create_dir_all(options.output).map_err(|error| {
        Error::new(
            codes::SOURCE_030,
            format!("cannot create {}: {error}", options.output.display()),
        )
    })?;
    let snapshot = options.output.join("RPMX-SOURCE.tar.gz");
    pack_snapshot(options.workspace, &snapshot, options.source_date_epoch)?;
    Ok(snapshot)
}

pub fn apply_patches(
    sandbox: &Sandbox<'_>,
    recipe_dir: &Path,
    patches: &[rpmx_language::model::Patch],
    workspace: &Path,
) -> Result<()> {
    if patches.is_empty() {
        return Ok(());
    }
    let recipe_dir = std::fs::canonicalize(recipe_dir).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot resolve {}: {error}", recipe_dir.display()),
        )
    })?;
    let patch_dir = workspace.join(".rpmx-patches");
    std::fs::create_dir_all(&patch_dir).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot create {}: {error}", patch_dir.display()),
        )
    })?;

    for (index, patch) in patches.iter().enumerate() {
        let source = std::fs::canonicalize(recipe_dir.join(&patch.file)).map_err(|error| {
            Error::new(
                codes::SOURCE_031,
                format!("cannot read patch '{}': {error}", patch.file),
            )
        })?;
        if !source.starts_with(&recipe_dir) {
            return Err(Error::new(
                codes::SOURCE_031,
                format!("patch '{}' escapes the recipe directory", patch.file),
            ));
        }
        let name = format!("{index}.patch");
        std::fs::copy(&source, patch_dir.join(&name)).map_err(|error| {
            Error::new(
                codes::SOURCE_031,
                format!("cannot stage patch '{}': {error}", patch.file),
            )
        })?;
        let step = rpmx_language::plan::PlannedStep {
            uses: format!("patch:{}", patch.file),
            command: Some("patch".to_string()),
            args: vec![
                format!("-p{}", patch.strip),
                "-i".to_string(),
                format!("{}/.rpmx-patches/{name}", rpmx_base::guest::BUILD),
            ],
            cwd: None,
            env: std::collections::BTreeMap::new(),
            needs: Vec::new(),
            tool_digest: None,
            step_digest: None,
            builtin: false,
            inputs: std::collections::BTreeMap::new(),
            input: None,
            input_file: None,
        };
        sandbox.run(&step)?;
    }

    std::fs::remove_dir_all(&patch_dir).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot remove {}: {error}", patch_dir.display()),
        )
    })?;
    Ok(())
}

fn materialize(
    path: &Path,
    name: &str,
    extract: Option<&rpmx_language::model::Extract>,
    workspace: &Path,
) -> Result<()> {
    let Some(extract) = extract else {
        let target = workspace.join(name);
        return std::fs::copy(path, &target).map(|_| ()).map_err(|error| {
            Error::new(
                codes::SOURCE_031,
                format!("cannot copy to {}: {error}", target.display()),
            )
        });
    };
    let format = match extract.format.as_deref().unwrap_or("auto") {
        "auto" => detect_format(path),
        other => other.to_string(),
    };
    if format == "none" {
        let target = workspace.join(name);
        return std::fs::copy(path, &target).map(|_| ()).map_err(|error| {
            Error::new(
                codes::SOURCE_031,
                format!("cannot copy to {}: {error}", target.display()),
            )
        });
    }
    std::fs::create_dir_all(workspace).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot create {}: {error}", workspace.display()),
        )
    })?;
    let file = std::fs::File::open(path).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot open {}: {error}", path.display()),
        )
    })?;
    let reader: Box<dyn std::io::Read> = match format.as_str() {
        "tar.gz" | "tgz" | "gzip" => Box::new(flate2::read::GzDecoder::new(file)),
        "tar.xz" | "xz" => Box::new(liblzma::read::XzDecoder::new(file)),
        "tar.zst" | "zstd" | "tar" => {
            if format == "tar" {
                Box::new(file)
            } else {
                Box::new(zstd::stream::read::Decoder::new(file).map_err(|error| {
                    Error::new(
                        codes::SOURCE_031,
                        format!("cannot read {}: {error}", path.display()),
                    )
                })?)
            }
        }
        other => {
            return Err(Error::new(
                codes::SOURCE_031,
                format!(
                    "unsupported extract format '{other}' for {}",
                    path.display()
                ),
            ))
        }
    };
    let mut staged = tempfile::tempfile_in(workspace).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot stage {}: {error}", path.display()),
        )
    })?;
    let subject = format!("archive {}", path.display());
    rpmx_base::io::copy_to(reader, &mut staged, rpmx_base::MAX_ARCHIVE_BYTES).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot stage {}: {error}", path.display()),
        )
    })?;
    rpmx_store::tar_guard::unpack_staged_with(
        staged,
        workspace,
        &subject,
        codes::SOURCE_031,
        None,
        extract.strip_components.unwrap_or(1),
        extract.destination.as_deref().unwrap_or("."),
    )
}

/// The source snapshot is always gzip-compressed; keep its own entry point.
fn unpack_gzip(path: &Path, workspace: &Path) -> Result<()> {
    let extract = rpmx_language::model::Extract {
        format: Some("tar.gz".to_string()),
        strip_components: Some(0),
        destination: Some(".".to_string()),
    };
    materialize(path, "RPMX-SOURCE.tar.gz", Some(&extract), workspace)
}

fn detect_format(path: &Path) -> String {
    use std::io::Read;
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

pub fn pack_snapshot(workspace: &Path, snapshot: &Path, source_date_epoch: u32) -> Result<()> {
    let files = package::scan_stage(workspace)?;
    let output = std::fs::File::create(snapshot).map_err(|error| {
        Error::new(
            codes::SOURCE_032,
            format!("cannot create {}: {error}", snapshot.display()),
        )
    })?;
    let gz = flate2::GzBuilder::new()
        .mtime(source_date_epoch)
        .write(output, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);

    for file in files {
        let name = file.path.trim_start_matches('/');
        if name == "RPMX-SOURCE.tar.gz" {
            continue;
        }
        let mut header = tar::Header::new_gnu();
        header.set_path(name).map_err(|error| {
            Error::new(codes::SOURCE_032, format!("invalid path {name}: {error}"))
        })?;
        header.set_mtime(u64::from(source_date_epoch));
        header.set_uid(0);
        header.set_gid(0);
        if let Some(target) = &file.target {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_link_name(target).map_err(|error| {
                Error::new(
                    codes::SOURCE_032,
                    format!("invalid symlink target for {name}: {error}"),
                )
            })?;
            header.set_cksum();
            tar.append(&header, std::io::empty()).map_err(|error| {
                Error::new(
                    codes::SOURCE_032,
                    format!("cannot add symlink {name} to snapshot: {error}"),
                )
            })?;
        } else {
            header.set_size(file.data.len() as u64);
            header.set_mode(file.mode);
            header.set_cksum();
            tar.append(&header, file.data.as_slice()).map_err(|error| {
                Error::new(
                    codes::SOURCE_032,
                    format!("cannot add {name} to snapshot: {error}"),
                )
            })?;
        }
    }

    let gz = tar
        .into_inner()
        .map_err(|error| Error::new(codes::SOURCE_032, format!("cannot finish tar: {error}")))?;
    let mut output = gz
        .finish()
        .map_err(|error| Error::new(codes::SOURCE_032, format!("cannot finish gzip: {error}")))?;
    output.flush().map_err(|error| {
        Error::new(codes::SOURCE_032, format!("cannot flush snapshot: {error}"))
    })?;
    Ok(())
}

pub fn unpack_snapshot(snapshot: &Path, workspace: &Path) -> Result<()> {
    unpack_gzip(snapshot, workspace)
}

#[cfg(test)]
mod extract_tests {
    use super::materialize;
    use rpmx_language::model::Extract;
    use std::io::Write;

    fn gzip_tar(path: &std::path::Path, entry: &str, bytes: &[u8]) {
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, entry, bytes).expect("entry");
        let tar = tar.into_inner().expect("tar");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar).expect("gzip");
        std::fs::write(path, gz.finish().expect("finish")).expect("write");
    }

    #[test]
    fn strip_components_puts_a_tarball_at_the_workspace_root() {
        let directory = tempfile::tempdir().expect("tempdir");
        let archive = directory.path().join("pkg.tar.gz");
        gzip_tar(&archive, "pkg-1.0/README", b"hello");
        let workspace = directory.path().join("workspace");
        let extract = Extract {
            format: Some("auto".to_string()),
            strip_components: Some(1),
            destination: Some(".".to_string()),
        };
        materialize(&archive, "pkg.tar.gz", Some(&extract), &workspace).expect("unpack");
        assert_eq!(
            std::fs::read_to_string(workspace.join("README")).expect("read"),
            "hello"
        );
    }

    #[test]
    fn a_destination_directory_is_honoured() {
        let directory = tempfile::tempdir().expect("tempdir");
        let archive = directory.path().join("pkg.tar.gz");
        gzip_tar(&archive, "README", b"hello");
        let workspace = directory.path().join("workspace");
        let extract = Extract {
            format: Some("tar.gz".to_string()),
            strip_components: Some(0),
            destination: Some("vendor".to_string()),
        };
        materialize(&archive, "pkg.tar.gz", Some(&extract), &workspace).expect("unpack");
        assert_eq!(
            std::fs::read_to_string(workspace.join("vendor/README")).expect("read"),
            "hello"
        );
    }

    #[test]
    fn magic_bytes_win_over_a_misleading_name() {
        let directory = tempfile::tempdir().expect("tempdir");
        let archive = directory.path().join("pkg.tar.gz");
        gzip_tar(&archive, "README", b"hello");
        let renamed = directory.path().join("pkg.data");
        std::fs::rename(&archive, &renamed).expect("rename");
        let workspace = directory.path().join("workspace");
        let extract = Extract {
            format: Some("auto".to_string()),
            strip_components: Some(0),
            destination: Some(".".to_string()),
        };
        materialize(&renamed, "pkg.data", Some(&extract), &workspace).expect("unpack");
        assert!(workspace.join("README").is_file());
    }
}
