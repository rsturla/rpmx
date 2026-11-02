//! Component inventory and bundled-provides discovery.
//!
//! Discovery reads vendor metadata; it never runs a package manager. The
//! inventory is the single source for `bundled(...)` provides and for the
//! component section of the future SBOM.

mod catalog;
mod discover;
mod model;
mod naming;

pub use discover::{discover, prepare_output_dirs};
pub use model::{
    CatalogerRef, Component, Confidence, Ecosystem, Evidence, Inventory, Scope, Source,
};
pub use naming::{cargo_version, go_version};

pub const SCHEMA: u32 = 1;
pub const BUNDLED_DIR: &str = ".rpmx/bundled";
pub const INVENTORY_FILE: &str = "RPMX-BUNDLED.json";
