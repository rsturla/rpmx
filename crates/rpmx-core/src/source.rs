use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{codes, Error, Result};
use crate::fetch::{fetch_sources, FetchOptions};
use crate::model::Recipe;
use crate::package;
use crate::plan::Plan;
use crate::sandbox::Sandbox;

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
    pub jobs: u32,
    pub step_timeout: u64,
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
            source.archive,
            options.workspace,
        )?;
    }

    let host_arch = crate::arch::Arch::current()?;
    let target_arch = crate::arch::Arch::parse(&options.plan.arch)?;
    let backend = options
        .runner
        .parse::<crate::sandbox::RunnerKind>()?
        .select(host_arch, target_arch, options.root, None)?;
    let sandbox = Sandbox {
        backend,
        workspace: options.workspace,
        stage: options.stage,
        host_arch,
        target_arch,
        source_date_epoch: options.source_date_epoch,
        jobs: options.jobs,
        timeout: Some(options.step_timeout).filter(|seconds| *seconds > 0),
        debuginfo: false,
        session: std::cell::RefCell::new(None),
    };

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
    patches: &[crate::model::Patch],
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
        let step = crate::plan::PlannedStep {
            uses: format!("patch:{}", patch.file),
            command: Some("patch".to_string()),
            args: vec![
                format!("-p{}", patch.strip),
                "-i".to_string(),
                format!("{}/.rpmx-patches/{name}", crate::guest::BUILD),
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

fn materialize(path: &Path, name: &str, archive: bool, workspace: &Path) -> Result<()> {
    let target = workspace.join(name);
    if archive {
        let file = std::fs::File::open(path).map_err(|error| {
            Error::new(
                codes::SOURCE_031,
                format!("cannot open {}: {error}", path.display()),
            )
        })?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(workspace).map_err(|error| {
            Error::new(
                codes::SOURCE_031,
                format!("cannot unpack {}: {error}", path.display()),
            )
        })?;
        return Ok(());
    }
    std::fs::copy(path, &target).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot copy to {}: {error}", target.display()),
        )
    })?;
    Ok(())
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
    let file = std::fs::File::open(snapshot).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot open {}: {error}", snapshot.display()),
        )
    })?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(workspace).map_err(|error| {
        Error::new(
            codes::SOURCE_031,
            format!("cannot unpack {}: {error}", snapshot.display()),
        )
    })
}
