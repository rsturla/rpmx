pub mod extract;
pub mod repo;
pub mod sign;

use std::path::Path;

use rpm::{BuildConfig, CompressionType, FileMode, FileOptions, PackageBuilder, Scriptlet};
use rpmx_core::package_spec::{BinaryPackageSpec, SourcePackageSpec};
use rpmx_core::{error::codes, Error};

/// The architecture recorded in an RPM or SRPM header.
pub fn package_arch(path: &Path) -> Result<String, Error> {
    let package = rpm::Package::open(path).map_err(|error| {
        Error::new(
            codes::EXTRACT_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    Ok(package.metadata.get_arch().unwrap_or("noarch").to_string())
}

pub fn write_srpm(spec: &SourcePackageSpec, out: &Path) -> Result<(), Error> {
    let mut builder = PackageBuilder::new(
        &spec.name,
        &spec.version,
        &spec.license,
        &spec.arch,
        &spec.summary,
    );

    builder
        .source_package(true)
        .release(&spec.release)
        .description(&spec.description)
        .using_config(
            BuildConfig::v4()
                .source_date(spec.source_date_epoch)
                .compression(CompressionType::Gzip),
        );

    if let Some(epoch) = spec.epoch {
        builder.epoch(epoch);
    }

    for file in &spec.files {
        let mode = u16::try_from(file.mode).map_err(|_| {
            Error::new(
                codes::RPM_004,
                format!("invalid mode {:o} for {}", file.mode, file.path),
            )
        })?;
        builder
            .with_file_contents(
                file.data.as_slice(),
                FileOptions::new(format!("./{}", file.path)).mode(FileMode::regular(mode)),
            )
            .map_err(|error| {
                Error::new(
                    codes::RPM_001,
                    format!("cannot add {} to source package: {error}", file.path),
                )
            })?;
    }

    let package = builder
        .build()
        .map_err(|error| Error::new(codes::RPM_002, format!("cannot build SRPM: {error}")))?;

    package.write_file(out).map_err(|error| {
        Error::new(
            codes::RPM_003,
            format!("cannot write {}: {error}", out.display()),
        )
    })
}

pub fn write_binary_rpm(spec: &BinaryPackageSpec, out: &Path) -> Result<(), Error> {
    let mut builder = PackageBuilder::new(
        &spec.name,
        &spec.version,
        &spec.license,
        &spec.arch,
        &spec.summary,
    );

    builder
        .release(&spec.release)
        .description(&spec.description)
        .source_rpm(&spec.source_rpm)
        .build_host("rpmx")
        .using_config(
            BuildConfig::v4()
                .source_date(spec.source_date_epoch)
                .compression(CompressionType::Gzip),
        );

    if let Some(epoch) = spec.epoch {
        builder.epoch(epoch);
    }
    if let Some(url) = &spec.url {
        builder.url(url);
    }

    for file in &spec.files {
        if let Some(target) = &file.target {
            let mut options = FileOptions::symlink(&file.path, target);
            if let Some(user) = &file.user {
                options = options.user(user);
            }
            if let Some(group) = &file.group {
                options = options.group(group);
            }
            builder.with_symlink(options).map_err(|error| {
                Error::new(
                    codes::RPM_001,
                    format!(
                        "cannot add symlink {} -> {target} to binary package: {error}",
                        file.path
                    ),
                )
            })?;
            continue;
        }
        let mode = u16::try_from(file.mode).map_err(|_| {
            Error::new(
                codes::RPM_004,
                format!("invalid mode {:o} for {}", file.mode, file.path),
            )
        })?;
        let mut options = FileOptions::new(&file.path).mode(FileMode::regular(mode));
        if file.config {
            options = options.config();
        }
        if file.noreplace {
            options = options.noreplace();
        }
        if file.doc {
            options = options.doc();
        }
        if file.license {
            options = options.license();
        }
        if let Some(user) = &file.user {
            options = options.user(user);
        }
        if let Some(group) = &file.group {
            options = options.group(group);
        }
        if let Some(caps) = &file.caps {
            options = options.caps(caps).map_err(|error| {
                Error::new(
                    codes::RPM_012,
                    format!("invalid capabilities for {}: {error}", file.path),
                )
            })?;
        }
        builder
            .with_file_contents(file.data.as_slice(), options)
            .map_err(|error| {
                Error::new(
                    codes::RPM_001,
                    format!("cannot add {} to binary package: {error}", file.path),
                )
            })?;
    }

    for dep in &spec.dependencies.requires {
        builder.requires(parse_dependency(dep)?);
    }
    for dep in &spec.dependencies.recommends {
        builder.recommends(parse_dependency(dep)?);
    }
    for dep in &spec.dependencies.suggests {
        builder.suggests(parse_dependency(dep)?);
    }
    for dep in &spec.dependencies.supplements {
        builder.supplements(parse_dependency(dep)?);
    }
    for dep in &spec.dependencies.enhances {
        builder.enhances(parse_dependency(dep)?);
    }
    for dep in &spec.dependencies.provides {
        builder.provides(parse_dependency(dep)?);
    }
    for dep in &spec.dependencies.obsoletes {
        builder.obsoletes(parse_dependency(dep)?);
    }
    for dep in &spec.dependencies.conflicts {
        builder.conflicts(parse_dependency(dep)?);
    }

    let scriptlet = |script: &String| {
        let mut value = Scriptlet::new(script.clone());
        if let Some(interpreter) = &spec.scriptlets.interpreter {
            value = value.prog(vec![interpreter.clone()]);
        }
        value
    };
    if let Some(script) = &spec.scriptlets.pre {
        builder.pre_install_script(scriptlet(script));
    }
    if let Some(script) = &spec.scriptlets.post {
        builder.post_install_script(scriptlet(script));
    }
    if let Some(script) = &spec.scriptlets.preun {
        builder.pre_uninstall_script(scriptlet(script));
    }
    if let Some(script) = &spec.scriptlets.postun {
        builder.post_uninstall_script(scriptlet(script));
    }

    for directory in &spec.directories {
        let mode = u16::try_from(directory.mode).map_err(|_| {
            Error::new(
                codes::RPM_004,
                format!(
                    "invalid mode {:o} for directory {}",
                    directory.mode, directory.path
                ),
            )
        })?;
        let mut options = FileOptions::dir(&directory.path).permissions(mode);
        if let Some(user) = &directory.user {
            options = options.user(user);
        }
        if let Some(group) = &directory.group {
            options = options.group(group);
        }
        builder.with_dir_entry(options).map_err(|error| {
            Error::new(
                codes::RPM_001,
                format!("cannot add directory {}: {error}", directory.path),
            )
        })?;
    }

    let package = builder
        .build()
        .map_err(|error| Error::new(codes::RPM_011, format!("cannot build RPM: {error}")))?;

    package.write_file(out).map_err(|error| {
        Error::new(
            codes::RPM_003,
            format!("cannot write {}: {error}", out.display()),
        )
    })
}

fn parse_dependency(text: &str) -> Result<rpm::Dependency, Error> {
    let text = text.trim();
    if text.starts_with('(') {
        return Err(Error::new(
            codes::RPM_012,
            format!("rich dependency '{text}' is not supported yet"),
        ));
    }
    let parts: Vec<&str> = text.split_whitespace().collect();
    match parts.as_slice() {
        [name] => Ok(rpm::Dependency::any(*name)),
        [name, "=", version] => Ok(rpm::Dependency::eq(*name, *version)),
        [name, ">=", version] => Ok(rpm::Dependency::greater_eq(*name, *version)),
        [name, ">", version] => Ok(rpm::Dependency::greater(*name, *version)),
        [name, "<=", version] => Ok(rpm::Dependency::less_eq(*name, *version)),
        [name, "<", version] => Ok(rpm::Dependency::less(*name, *version)),
        _ => Err(Error::new(
            codes::RPM_013,
            format!("cannot parse dependency '{text}'"),
        )),
    }
}
