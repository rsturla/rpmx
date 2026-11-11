use crate::domain::{error::codes, Error};
pub(crate) use crate::emit;
pub use crate::policy::{reconcile_policy, resolve_policy, PlanPolicy, PolicyInput};
pub(crate) use crate::{autoroot, package_lock as lock};
pub use rpmx_build::source::source_date_epoch;
pub(crate) use rpmx_root::recipe::{load_plan, step_root};
pub use rpmx_rpm::sign::{load_signing_key, signature_timestamp};
use std::path::PathBuf;

pub fn default_cache() -> Result<PathBuf, Error> {
    if let Ok(base) = std::env::var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(base).join("rpmx/cas"));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Ok(PathBuf::from(home).join(".cache/rpmx/cas"));
    }
    Err(Error::new(
        codes::FETCH_005,
        "cannot determine cache directory; pass --cache",
    ))
}

pub(crate) use rpmx_base::io::WorkDir;
