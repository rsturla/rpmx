pub mod arch;
pub mod error;
pub mod guest;
pub mod hash;
pub mod io;
pub mod output;
pub mod yaml;

pub use arch::Arch;
pub use error::{Error, Result};

/// Hard ceiling for an untrusted archive staged or downloaded by the tool.
pub const MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
