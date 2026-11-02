use std::path::Path;

use rpmx_core::{error::codes, Error};

pub use rpmx_core::srpm::ExtractedBundle as ExtractedSrpm;

/// Extract an SRPM's payload and read its embedded bundle.
pub fn extract_srpm(path: &Path, dest: &Path) -> Result<ExtractedSrpm, Error> {
    let package = rpm::Package::open(path).map_err(|error| {
        Error::new(
            codes::EXTRACT_001,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let files = rpmx_payload::extract_payload(&package, dest)?;
    rpmx_core::srpm::read_bundle(dest, files)
}
