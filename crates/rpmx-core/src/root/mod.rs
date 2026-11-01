mod assemble;
mod container;
mod lock;
mod resolve;

pub use assemble::{assemble, remove_tree, AssembleOptions};
pub use lock::{
    closure_digest, load_lock, write_lock, LockedBuilder, LockedPackage, RootLock, RootSelection,
    DOCUMENT, VERSION,
};
pub use resolve::resolve_profile;
