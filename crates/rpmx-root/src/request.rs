use std::path::{Path, PathBuf};

use rpmx_base::error::{codes, Error, Result};

use crate::root::{RootLock, RootSelection};

pub enum RootRequest {
    Host,
    Locked {
        lock: Box<RootLock>,
        path: PathBuf,
        digest: String,
    },
}

pub struct RootRequestOptions<'a> {
    pub lock: Option<&'a Path>,
    pub root: Option<&'a Path>,
    pub allow_host: bool,
    pub arch: &'a str,
    /// Re-hash the tree and compare it with the lock. Explicit roots verify;
    /// a root that this process just resolved or took from its own cache does
    /// not need to hash every file again.
    pub verify_digest: bool,
}

/// Resolve the root/root-lock/unsafe-host combination once for command handlers.
pub fn resolve(options: RootRequestOptions<'_>) -> Result<RootRequest> {
    match (options.lock, options.root) {
        (Some(lock_path), Some(root_path)) => {
            if options.allow_host {
                return Err(Error::new(
                    codes::ROOT_016,
                    "--unsafe-host-root cannot be combined with --root-lock or --root",
                ));
            }
            let selection = if options.verify_digest {
                RootSelection::load(lock_path, root_path, options.arch)?
            } else {
                let lock = crate::root::load_lock(lock_path)?;
                lock.require_target(options.arch)?;
                let digest = lock.require_digest()?.to_string();
                let path = std::fs::canonicalize(root_path).map_err(|error| {
                    Error::new(
                        codes::ROOT_017,
                        format!("cannot resolve {}: {error}", root_path.display()),
                    )
                })?;
                RootSelection { lock, path, digest }
            };
            Ok(RootRequest::Locked {
                lock: Box::new(selection.lock),
                path: selection.path,
                digest: selection.digest,
            })
        }
        (Some(_), None) => Err(Error::new(codes::ROOT_016, "--root-lock requires --root")),
        (None, Some(_)) => Err(Error::new(codes::ROOT_016, "--root requires --root-lock")),
        (None, None) if options.allow_host => Ok(RootRequest::Host),
        (None, None) => Err(Error::new(
            codes::ROOT_016,
            "a locked root is required; pass --root-lock and --root, \
             or --unsafe-host-root",
        )),
    }
}
