use sha2::{Digest, Sha256, Sha512};

/// The canonical content digest used by locks, CAS paths and SBOMs.
pub fn sha512_hex(bytes: &[u8]) -> String {
    format!("sha512:{:x}", Sha512::digest(bytes))
}

/// A source digest published by an upstream that does not offer SHA-512.
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Hash a file with SHA-256 without loading it into memory.
pub fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Hash a file without loading it into memory.
pub fn sha512_file(path: &std::path::Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha512::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("sha512:{:x}", hasher.finalize()))
}
