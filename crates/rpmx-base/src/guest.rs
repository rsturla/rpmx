//! Paths and variables that define the guest-side build contract.
//!
//! The host workspace and stage are mounted or copied to these locations.
//! Recipes, runners and patch staging must all use the same values.

pub const BUILD: &str = "/tmp/build";
pub const STAGE: &str = "/tmp/stage";
pub const PREFIX: &str = "/usr";
pub const LIBDIR: &str = "/usr/lib64";
