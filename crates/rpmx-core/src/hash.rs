use sha2::{Digest, Sha512};

/// The canonical content digest used by locks, CAS paths and SBOMs.
pub fn sha512_hex(bytes: &[u8]) -> String {
    format!("sha512:{:x}", Sha512::digest(bytes))
}
