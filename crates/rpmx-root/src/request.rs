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
            let selection = RootSelection::load(lock_path, root_path, options.arch)?;
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
