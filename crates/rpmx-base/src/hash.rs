use sha2::{Digest, Sha512};

/// The canonical content digest used by locks, CAS paths and SBOMs.
pub fn sha512_hex(bytes: &[u8]) -> String {
    format!("sha512:{:x}", Sha512::digest(bytes))
}

/// Hash a file without loading it into memory.
pub fn sha512_file(path: &std::path::Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha512::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("sha512:{:x}", hasher.finalize()))
}
