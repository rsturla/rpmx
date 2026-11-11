use std::path::Path;

use rpmx_base::error::{codes, Error, Result};

use crate::root::RootSelection;

pub enum RootRequest {
    Host,
    Locked(Box<RootSelection>),
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
            Ok(RootRequest::Locked(Box::new(selection)))
        }
        (Some(_), None) => Err(Error::new(codes::ROOT_016, "--root-lock requires --root")),
        (None, Some(_)) => Err(Error::new(codes::ROOT_016, "--root requires --root-lock")),
        (None, None) if options.allow_host => Ok(RootRequest::Host),
        (None, None) => Err(Error::new(
            codes::ROOT_016,
            "no build root: add `build.root` to the recipe, pass --profile FILE, \
             pass --root-lock and --root, or use --unsafe-host-root",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve, RootRequestOptions};

    #[test]
    fn a_missing_root_points_at_the_recipe_field() {
        let error = match resolve(RootRequestOptions {
            lock: None,
            root: None,
            allow_host: false,
            arch: "x86_64",
        }) {
            Ok(_) => panic!("a root was resolved without any input"),
            Err(error) => error,
        };
        assert!(error.message.contains("build.root"), "{}", error.message);
        assert!(error.message.contains("--profile"), "{}", error.message);
    }
}
