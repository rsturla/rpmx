use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::error::{codes, Error, Result};

#[derive(Debug, Clone)]
pub struct StagedFile {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
    /// Target when this entry is a symbolic link; `data` is empty then.
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OutputSpec {
    pub name: String,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub metapackage: bool,
}

#[derive(Debug, Clone)]
pub struct SplitOutput {
    pub name: String,
    pub files: Vec<StagedFile>,
}

pub fn scan_stage(root: &Path) -> Result<Vec<StagedFile>> {
    if !root.is_dir() {
        return Err(Error::new(
            codes::STAGE_001,
            format!("stage directory {} does not exist", root.display()),
        ));
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<StagedFile>) -> Result<()> {
    let mut entries = std::fs::read_dir(dir)
        .map_err(|error| {
            Error::new(
                codes::STAGE_002,
                format!("cannot read {}: {error}", dir.display()),
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            Error::new(
                codes::STAGE_002,
                format!("cannot read {}: {error}", dir.display()),
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            Error::new(
                codes::STAGE_002,
                format!("cannot stat {}: {error}", path.display()),
            )
        })?;
        if file_type.is_dir() {
            walk(root, &path, files)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| Error::new(codes::STAGE_005, format!("cannot relativize: {error}")))?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        if file_type.is_symlink() {
            let target = std::fs::read_link(&path).map_err(|error| {
                Error::new(
                    codes::STAGE_006,
                    format!("cannot read symlink {}: {error}", path.display()),
                )
            })?;
            let target = target.to_str().ok_or_else(|| {
                Error::new(
                    codes::STAGE_003,
                    format!("symlink {} has a non-UTF-8 target", path.display()),
                )
            })?;
            files.push(StagedFile {
                path: format!("/{relative}"),
                data: Vec::new(),
                mode: 0o777,
                target: Some(target.replace('\\', "/")),
            });
            continue;
        }
        if !file_type.is_file() {
            return Err(Error::new(
                codes::STAGE_004,
                format!("{} is not a regular file", path.display()),
            ));
        }
        let data = std::fs::read(&path).map_err(|error| {
            Error::new(
                codes::STAGE_006,
                format!("cannot read {}: {error}", path.display()),
            )
        })?;
        files.push(StagedFile {
            path: format!("/{relative}"),
            data,
            mode: mode_of(&path)?,
            target: None,
        });
    }
    Ok(())
}

#[cfg(unix)]
fn mode_of(path: &Path) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path).map_err(|error| {
        Error::new(
            codes::STAGE_007,
            format!("cannot stat {}: {error}", path.display()),
        )
    })?;
    Ok(metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode_of(_path: &Path) -> Result<u32> {
    Ok(0o644)
}

/// Compile an optional pattern list for callers that classify files after the
/// split (license, doc, config).
pub fn matcher(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        Ok(None)
    } else {
        Ok(Some(build_set(patterns)?))
    }
}

pub fn split(stage: &[StagedFile], outputs: &[OutputSpec]) -> Result<Vec<SplitOutput>> {
    let sets = outputs
        .iter()
        .map(|output| {
            Ok((
                build_set(&output.include)?,
                if output.exclude.is_empty() {
                    None
                } else {
                    Some(build_set(&output.exclude)?)
                },
            ))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut assigned: Vec<Vec<StagedFile>> = vec![Vec::new(); outputs.len()];
    for file in stage {
        let mut matches = Vec::new();
        for (index, (include, exclude)) in sets.iter().enumerate() {
            if include.is_match(&file.path)
                && !exclude
                    .as_ref()
                    .is_some_and(|exclude| exclude.is_match(&file.path))
            {
                matches.push(index);
            }
        }

        match matches.as_slice() {
            [] => {
                return Err(Error::new(
                    codes::OUTPUT_010,
                    format!("file '{}' is not claimed by any output", file.path),
                ));
            }
            [index] => assigned[*index].push(file.clone()),
            _ => {
                let names = matches
                    .iter()
                    .map(|index| outputs[*index].name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(Error::new(
                    codes::OUTPUT_011,
                    format!(
                        "file '{}' is claimed by multiple outputs: {names}",
                        file.path
                    ),
                ));
            }
        }
    }

    let mut split = Vec::with_capacity(outputs.len());
    for (index, output) in outputs.iter().enumerate() {
        if assigned[index].is_empty() && !output.metapackage {
            return Err(Error::new(
                codes::OUTPUT_012,
                format!("output '{}' matched no staged files", output.name),
            ));
        }
        split.push(SplitOutput {
            name: output.name.clone(),
            files: std::mem::take(&mut assigned[index]),
        });
    }
    Ok(split)
}

fn build_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|error| {
                Error::new(
                    codes::OUTPUT_013,
                    format!("invalid file pattern '{pattern}': {error}"),
                )
            })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| Error::new(codes::OUTPUT_014, error.to_string()))
}
