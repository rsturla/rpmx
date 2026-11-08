use std::io::Write;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

use crate::fetch::{fetch_sources, FetchOptions, FetchedSource};
use rpmx_base::error::{codes, Error, Result};
use rpmx_language::model::Recipe;
use rpmx_language::plan::Plan;
use rpmx_runner::runner::{RunnerAssets, RunnerKind};
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
    pub prefix: &'a str,
    pub libdir: &'a str,
    pub output: &'a Path,
    /// Explicit timestamp; when absent, use the recipe, source archives or
    /// the repository-wide fallback.
    pub source_date_epoch: Option<u32>,
    pub source_date_epoch_default: Option<u32>,
    pub jobs: NonZeroU32,
    pub step_timeout: NonZeroU64,
    pub root: Option<&'a Path>,
    pub appliance: Option<&'a Path>,
    pub exec: rpmx_base::arch::Arch,
    pub sysroot: Option<&'a Path>,
    pub cross: Option<&'a rpmx_base::guest::CrossToolchain>,
    pub runner: &'a str,
    pub recipe_dir: &'a Path,
}

/// The newest timestamp available from an explicit value, the recipe, or the
/// source archives. Archives are the reproducible fallback: a release tarball
/// keeps its upstream mtimes, and rpmx gives a git archive its commit time.
pub fn resolve_source_date_epoch(
    recipe: &Recipe,
    fetched: &[FetchedSource],
    explicit: Option<u32>,
    fallback: Option<u32>,
) -> u32 {
    explicit
        .or(recipe.package.source_date_epoch)
        .or_else(|| {
            fetched
                .iter()
                .filter_map(|source| source.source_date_epoch)
                .max()
        })
        .or(fallback)
        .unwrap_or(0)
}

pub struct AssembledSource {
    pub snapshot: PathBuf,
    pub source_date_epoch: u32,
}

pub fn assemble(options: SourceOptions<'_>) -> Result<AssembledSource> {
    let fetched = fetch_sources(
        options.recipe,
        FetchOptions {
            cache: options.cache,
            proxy: options.proxy,
            offline: options.offline,
            base_dir: options.base_dir,
        },
    )?;
    let source_date_epoch = resolve_source_date_epoch(
        options.recipe,
        &fetched,
        options.source_date_epoch,
        options.source_date_epoch_default,
    );
    let snapshot = assemble_fetched(&options, &fetched, source_date_epoch)?;
    Ok(AssembledSource {
        snapshot,
        source_date_epoch,
    })
}

/// Assemble an already-fetched source set with an explicit effective timestamp.
pub fn assemble_fetched(
    options: &SourceOptions<'_>,
    fetched: &[FetchedSource],
    source_date_epoch: u32,
) -> Result<PathBuf> {
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

    for source in fetched {
        materialize(
            &source.path,
            &source.name,
            source.extract.as_ref(),
            options.workspace,
        )?;
    }

    stage_context(
        options.recipe.context.as_deref(),
        options.recipe_dir,
        options.workspace,
    )?;

    let host_arch = rpmx_base::arch::Arch::current()?;
    let target_arch = rpmx_base::arch::Arch::parse(&options.plan.arch)?;
    let sandbox = Sandbox::new(SandboxConfig {
        runner: options.runner.parse::<RunnerKind>()?,
        host: host_arch,
        exec: options.exec,
        target: target_arch,
        sysroot: options.sysroot,
        cross: options.cross,
        workspace: options.workspace,
        stage: options.stage,
        root: options.root,
        assets: RunnerAssets::new(options.appliance, &[]),
        image: None,
        prefix: options.prefix,
        libdir: options.libdir,
        source_date_epoch,
        jobs: options.jobs,
        timeout: options.step_timeout,
    })?;

    let cells = options.plan.cells.iter().collect::<Vec<_>>();
    rpmx_components::prepare_output_dirs(options.workspace, &cells)?;

    apply_patches(
        &sandbox,
        options.recipe_dir,
        &options.recipe.patches,
        options.workspace,
    )?;

    if let Some(source_assembly) = &options.plan.source_assembly {
        let total = source_assembly.steps.len();
        for (index, step) in source_assembly.steps.iter().enumerate() {
            sandbox.run_numbered(step, index + 1, total)?;
        }
    }

    sandbox.sync()?;

    std::fs::create_dir_all(options.output).map_err(|error| {
        Error::new(
            codes::SOURCE_030,
            format!("cannot create {}: {error}", options.output.display()),
        )
    })?;
    let inventory = rpmx_components::discover(options.workspace, &cells)?;
    inventory.warn_declared();
    inventory.write(&options.output.join(rpmx_components::INVENTORY_FILE))?;
    let snapshot = options.output.join("RPMX-SOURCE.tar.gz");
    pack_snapshot(options.workspace, &snapshot, source_date_epoch)?;
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
            label: format!("patch:{}", patch.file),
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
            needs_tools: Vec::new(),
            needs_host: Vec::new(),
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

/// Resolve and validate the recipe's context directory.
///
/// The path must be a relative subdirectory of the recipe directory. The
/// contents are copied to the workspace root, matching melange's `--source-dir`.
pub fn context_root(recipe_dir: &Path, context: &str) -> Result<PathBuf> {
    use std::path::Component;

    let relative = Path::new(context);
    if context.is_empty() || relative.is_absolute() {
        return Err(Error::new(
            codes::SOURCE_033,
            format!("context '{context}' must be a relative directory"),
        ));
    }
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::new(
                    codes::SOURCE_033,
                    format!("context '{context}' must not contain '..' or a root component"),
                ));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(Error::new(
            codes::SOURCE_033,
            format!("context '{context}' must name a subdirectory of the recipe"),
        ));
    }

    let recipe_dir = std::fs::canonicalize(recipe_dir).map_err(|error| {
        Error::new(
            codes::SOURCE_033,
            format!(
                "cannot resolve recipe directory {}: {error}",
                recipe_dir.display()
            ),
        )
    })?;
    let root = std::fs::canonicalize(recipe_dir.join(normalized)).map_err(|error| {
        Error::new(
            codes::SOURCE_033,
            format!("cannot resolve context '{context}': {error}"),
        )
    })?;
    if root == recipe_dir || !root.starts_with(&recipe_dir) {
        return Err(Error::new(
            codes::SOURCE_033,
            format!("context '{context}' escapes the recipe directory"),
        ));
    }
    if !root.is_dir() {
        return Err(Error::new(
            codes::SOURCE_033,
            format!("context '{}' is not a directory", root.display()),
        ));
    }
    Ok(root)
}

/// Copy the recipe's context directory into the source workspace.
pub fn stage_context(context: Option<&str>, recipe_dir: &Path, workspace: &Path) -> Result<()> {
    let Some(context) = context else {
        return Ok(());
    };
    let root = context_root(recipe_dir, context)?;
    copy_context_tree(&root, &root, workspace)
}

fn copy_context_tree(root: &Path, directory: &Path, workspace: &Path) -> Result<()> {
    use std::path::Component;

    let relative = directory.strip_prefix(root).map_err(|error| {
        Error::new(
            codes::SOURCE_035,
            format!(
                "cannot relativize context path {}: {error}",
                directory.display()
            ),
        )
    })?;
    let entries = rpmx_base::io::sorted_entries(directory, codes::SOURCE_035)?;

    for entry in entries {
        let path = entry.path();
        let target = workspace.join(relative).join(entry.file_name());
        if target.symlink_metadata().is_ok() {
            return Err(Error::new(
                codes::SOURCE_034,
                format!(
                    "context entry {} collides with an upstream source",
                    target.display()
                ),
            ));
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            Error::new(
                codes::SOURCE_035,
                format!("cannot stat {}: {error}", path.display()),
            )
        })?;
        let file_type = metadata.file_type();

        if file_type.is_dir() {
            std::fs::create_dir(&target).map_err(|error| {
                Error::new(
                    codes::SOURCE_035,
                    format!("cannot create {}: {error}", target.display()),
                )
            })?;
            copy_context_tree(root, &path, workspace)?;
            set_context_mode(&target, &metadata)?;
        } else if file_type.is_symlink() {
            let link = std::fs::read_link(&path).map_err(|error| {
                Error::new(
                    codes::SOURCE_035,
                    format!("cannot read symlink {}: {error}", path.display()),
                )
            })?;
            if link.is_absolute()
                || link.components().any(|component| {
                    matches!(
                        component,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
            {
                return Err(Error::new(
                    codes::SOURCE_035,
                    format!(
                        "context symlink {} must be relative and stay inside the context",
                        path.display()
                    ),
                ));
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link, &target).map_err(|error| {
                Error::new(
                    codes::SOURCE_035,
                    format!("cannot create symlink {}: {error}", target.display()),
                )
            })?;
            #[cfg(not(unix))]
            return Err(Error::new(
                codes::SOURCE_035,
                format!("symlinks are not supported in context {}", path.display()),
            ));
        } else if file_type.is_file() {
            std::fs::copy(&path, &target).map_err(|error| {
                Error::new(
                    codes::SOURCE_035,
                    format!(
                        "cannot copy {} to {}: {error}",
                        path.display(),
                        target.display()
                    ),
                )
            })?;
            set_context_mode(&target, &metadata)?;
        } else {
            return Err(Error::new(
                codes::SOURCE_035,
                format!(
                    "context entry {} is not a regular file, directory or symlink",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_context_mode(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = metadata.permissions().mode() & 0o7777;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|error| {
        Error::new(
            codes::SOURCE_035,
            format!("cannot set mode on {}: {error}", path.display()),
        )
    })
}

#[cfg(not(unix))]
fn set_context_mode(_path: &Path, _metadata: &std::fs::Metadata) -> Result<()> {
    Ok(())
}

fn materialize(
    path: &Path,
    name: &str,
    extract: Option<&rpmx_language::model::Extract>,
    workspace: &Path,
) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(Error::new(
            codes::SOURCE_031,
            format!("source name '{name}' must be a single path component"),
        ));
    }
    let Some(extract) = extract else {
        let target = workspace.join(name);
        return std::fs::copy(path, &target).map(|_| ()).map_err(|error| {
            Error::new(
                codes::SOURCE_031,
                format!("cannot copy to {}: {error}", target.display()),
            )
        });
    };
    if let Some(destination) = &extract.destination {
        if destination.starts_with('/') || destination.split(['/', '\\']).any(|part| part == "..") {
            return Err(Error::new(
                codes::SOURCE_031,
                format!("extract destination '{destination}' must stay under the workspace"),
            ));
        }
    }
    let format = crate::archive::format(path, Some(extract));
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
    let reader = crate::archive::reader(path, &format)?;
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

pub fn pack_snapshot(workspace: &Path, snapshot: &Path, source_date_epoch: u32) -> Result<()> {
    let files = snapshot_entries(workspace)?;
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
        let name = file.path.as_str();
        if name == "RPMX-SOURCE.tar.gz" {
            continue;
        }
        let mut header = tar::Header::new_gnu();
        header.set_mtime(u64::from(source_date_epoch));
        header.set_uid(0);
        header.set_gid(0);
        if file.directory {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(file.mode);
            tar.append_data(&mut header, name, std::io::empty())
                .map_err(|error| {
                    Error::new(
                        codes::SOURCE_032,
                        format!("cannot add directory {name} to snapshot: {error}"),
                    )
                })?;
        } else if let Some(target) = &file.target {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            tar.append_link(&mut header, name, target)
                .map_err(|error| {
                    Error::new(
                        codes::SOURCE_032,
                        format!("cannot add symlink {name} to snapshot: {error}"),
                    )
                })?;
        } else {
            header.set_size(file.data.len() as u64);
            header.set_mode(file.mode);
            tar.append_data(&mut header, name, file.data.as_slice())
                .map_err(|error| {
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

struct SnapshotEntry {
    path: String,
    mode: u32,
    data: Vec<u8>,
    target: Option<String>,
    directory: bool,
}

fn snapshot_entries(root: &Path) -> Result<Vec<SnapshotEntry>> {
    let mut entries = Vec::new();
    snapshot_walk(root, root, &mut entries)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn snapshot_walk(root: &Path, directory: &Path, entries: &mut Vec<SnapshotEntry>) -> Result<()> {
    let children = rpmx_base::io::sorted_entries(directory, codes::SOURCE_032)?;
    for child in children {
        let path = child.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| Error::new(codes::SOURCE_032, format!("cannot relativize: {error}")))?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            Error::new(
                codes::SOURCE_032,
                format!("cannot stat {}: {error}", path.display()),
            )
        })?;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            entries.push(SnapshotEntry {
                path: relative,
                mode: snapshot_mode(&metadata),
                data: Vec::new(),
                target: None,
                directory: true,
            });
            snapshot_walk(root, &path, entries)?;
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(&path).map_err(|error| {
                Error::new(
                    codes::SOURCE_032,
                    format!("cannot read symlink {}: {error}", path.display()),
                )
            })?;
            entries.push(SnapshotEntry {
                path: relative,
                mode: 0o777,
                data: Vec::new(),
                target: Some(target.to_string_lossy().replace('\\', "/")),
                directory: false,
            });
        } else if file_type.is_file() {
            let data = std::fs::read(&path).map_err(|error| {
                Error::new(
                    codes::SOURCE_032,
                    format!("cannot read {}: {error}", path.display()),
                )
            })?;
            entries.push(SnapshotEntry {
                path: relative,
                mode: snapshot_mode(&metadata),
                data,
                target: None,
                directory: false,
            });
        } else {
            return Err(Error::new(
                codes::SOURCE_032,
                format!(
                    "{} is not a regular file, directory or symlink",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn snapshot_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn snapshot_mode(_metadata: &std::fs::Metadata) -> u32 {
    0o644
}

pub fn unpack_snapshot(snapshot: &Path, workspace: &Path) -> Result<()> {
    unpack_gzip(snapshot, workspace)
}

#[cfg(test)]
mod extract_tests {
    use super::{materialize, pack_snapshot, unpack_snapshot};
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
    fn snapshot_packs_paths_longer_than_the_gnu_name_field() {
        let directory = tempfile::tempdir().expect("tempdir");
        let workspace = directory.path().join("workspace");
        let nested = workspace.join("a".repeat(90));
        std::fs::create_dir_all(&nested).expect("mkdir");
        let name = format!("{}.txt", "b".repeat(90));
        std::fs::write(nested.join(&name), b"long path").expect("write");

        let snapshot = directory.path().join("source.tar.gz");
        pack_snapshot(&workspace, &snapshot, 1_704_067_200).expect("pack");

        let unpacked = directory.path().join("unpacked");
        unpack_snapshot(&snapshot, &unpacked).expect("unpack");
        assert_eq!(
            std::fs::read_to_string(unpacked.join("a".repeat(90)).join(name)).expect("read"),
            "long path"
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

#[cfg(test)]
mod tests {
    use super::*;
    use rpmx_language::model::{Package, Recipe};
    use std::collections::BTreeMap;

    fn recipe(source_date_epoch: Option<u32>) -> Recipe {
        Recipe {
            schema: 1,
            package: Package {
                name: "demo".to_string(),
                epoch: None,
                version: "1".to_string(),
                release: "1".to_string(),
                summary: "demo".to_string(),
                description: None,
                license: "MIT".to_string(),
                source_date_epoch,
                upstream: None,
                vcs: None,
                cpe: None,
                maintainers: Vec::new(),
            },
            context: None,
            vars: BTreeMap::new(),
            build: Default::default(),
            sources: Vec::new(),
            patches: Vec::new(),
            source_assembly: Default::default(),
            matrix: None,
            outputs: Vec::new(),
            tests: Vec::new(),
        }
    }

    fn fetched(source_date_epoch: Option<u32>) -> FetchedSource {
        FetchedSource {
            name: "demo".to_string(),
            digest: "sha512:00".to_string(),
            path: PathBuf::new(),
            extract: None,
            source_date_epoch,
        }
    }

    #[test]
    fn source_date_epoch_prefers_explicit_then_recipe_then_sources_then_default() {
        let recipe = recipe(Some(30));
        assert_eq!(
            resolve_source_date_epoch(&recipe, &[fetched(Some(40))], Some(20), Some(10)),
            20
        );
        assert_eq!(
            resolve_source_date_epoch(&recipe, &[fetched(Some(40))], None, Some(10)),
            30
        );
        let recipe = self::recipe(None);
        assert_eq!(
            resolve_source_date_epoch(
                &recipe,
                &[fetched(Some(40)), fetched(Some(50))],
                None,
                Some(10)
            ),
            50
        );
        assert_eq!(
            resolve_source_date_epoch(&recipe, &[fetched(None)], None, Some(10)),
            10
        );
    }
}
