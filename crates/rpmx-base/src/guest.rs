//! Paths and variables that define the guest-side build contract.
//!
//! The host workspace and stage are mounted or copied to these locations.
//! Recipes, runners and patch staging must all use the same values.

pub const BUILD: &str = "/tmp/build";
pub const STAGE: &str = "/tmp/stage";
pub const PREFIX: &str = "/usr";
pub const LIBDIR: &str = "/usr/lib64";

/// Where a cross build mounts the target sysroot, read-only.
///
/// It is a fixed guest path so a recipe never has to know where the sysroot
/// lives on the host. A profile cannot move it, for the same reason it cannot
/// move `STAGE`.
pub const SYSROOT: &str = "/sysroot";

/// The commands a cross toolchain provides.
///
/// The default names follow the GNU convention, `<triple>-gcc`. A distribution
/// that names its toolchain differently overrides them in the profile, so this
/// type never guesses a package layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossToolchain {
    pub triple: String,
    pub cc: String,
    pub cxx: String,
    pub ar: String,
    pub ld: String,
    pub strip: String,
    pub objcopy: String,
    pub pkg_config: String,
}

impl CrossToolchain {
    /// The GNU default: every command is `<triple>-<tool>`.
    pub fn new(triple: &str) -> Self {
        Self {
            triple: triple.to_string(),
            cc: format!("{triple}-gcc"),
            cxx: format!("{triple}-g++"),
            ar: format!("{triple}-ar"),
            ld: format!("{triple}-ld"),
            strip: format!("{triple}-strip"),
            objcopy: format!("{triple}-objcopy"),
            pkg_config: "pkg-config".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_commands_follow_the_gnu_triple() {
        let toolchain = CrossToolchain::new("aarch64-linux-gnu");
        assert_eq!(toolchain.cc, "aarch64-linux-gnu-gcc");
        assert_eq!(toolchain.cxx, "aarch64-linux-gnu-g++");
        assert_eq!(toolchain.ar, "aarch64-linux-gnu-ar");
        assert_eq!(toolchain.strip, "aarch64-linux-gnu-strip");
        assert_eq!(toolchain.pkg_config, "pkg-config");
    }

    #[test]
    fn the_sysroot_is_a_fixed_guest_path() {
        assert!(SYSROOT.starts_with('/'));
        assert_ne!(SYSROOT, STAGE);
    }
}
