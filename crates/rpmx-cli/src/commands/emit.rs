use std::path::{Path, PathBuf};

use rpmx_core::package_spec::{BinaryPackageSpec, SourcePackageSpec};
use rpmx_core::Error;
use rpmx_rpm::sign::SigningKey;
use rpmx_rpm::{write_binary_rpm, write_srpm};

/// Write and optionally sign one prepared binary package.
pub fn binary(
    package: &BinaryPackageSpec,
    output: &Path,
    signing: Option<&SigningKey>,
    timestamp: Option<u32>,
) -> Result<PathBuf, Error> {
    let path = output.join(format!(
        "{}-{}-{}.{}.rpm",
        package.name, package.version, package.release, package.arch
    ));
    write_binary_rpm(package, &path)?;
    if let Some(key) = signing {
        rpmx_rpm::sign::sign_package(&path, key, timestamp)?;
    }
    Ok(path)
}

/// Write and optionally sign one prepared source package.
pub fn source(
    package: &SourcePackageSpec,
    output: &Path,
    signing: Option<&SigningKey>,
    timestamp: Option<u32>,
) -> Result<PathBuf, Error> {
    let path = output.join(format!(
        "{}-{}-{}.src.rpm",
        package.name, package.version, package.release
    ));
    write_srpm(package, &path)?;
    if let Some(key) = signing {
        rpmx_rpm::sign::sign_package(&path, key, timestamp)?;
    }
    Ok(path)
}
