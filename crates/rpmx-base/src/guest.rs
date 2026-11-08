//! Paths and variables that define the guest-side build contract.
//!
//! The host workspace and stage are mounted or copied to these locations.
//! Recipes, runners and patch staging must all use the same values.

pub const BUILD: &str = "/tmp/build";
pub const STAGE: &str = "/tmp/stage";
/// Root for explicit bundled declarations written by source/build steps.
pub const BUNDLED_DIR: &str = "/tmp/build/.rpmx/bundled";
pub const PREFIX: &str = "/usr";
pub const LIBDIR: &str = "/usr/lib64";

/// Where a cross build mounts the target sysroot, read-only.
///
/// It is a fixed guest path so a recipe never has to know where the sysroot
/// lives on the host. A profile cannot move it, for the same reason it cannot
/// move `STAGE`.
pub const SYSROOT: &str = "/sysroot";

/// Toolchain flags every step sees, after the profile has resolved them for
/// one architecture.
///
/// They are ordinary compiler and linker arguments, not a policy enum: a
/// distribution can harden a native build with the same fields it uses for a
/// cross build. The sandbox places them before a recipe's own flags so a
/// recipe can add to, but not silently erase, its distribution's defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildFlags {
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
}

impl BuildFlags {
    pub fn is_empty(&self) -> bool {
        self.cflags.is_empty() && self.cxxflags.is_empty() && self.ldflags.is_empty()
    }
}

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
    /// True when the commands are the seed's native tools rather than
    /// `<triple>-*` wrappers. A native bootstrap still passes `--sysroot`, but
    /// `gcc` already targets the same architecture as the sysroot.
    pub native: bool,
    /// Distribution-specific flags appended to every command.
    pub cflags: Vec<String>,
    pub cxxflags: Vec<String>,
    pub ldflags: Vec<String>,
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
            native: false,
            cflags: Vec::new(),
            cxxflags: Vec::new(),
            ldflags: Vec::new(),
        }
    }

    /// Native command names plus the target triple.
    ///
    /// This is the seed-compiler form of a native bootstrap: `gcc` targets the
    /// same architecture as the sysroot, and the sandbox supplies
    /// `--sysroot` through `CFLAGS`/`LDFLAGS`.
    pub fn native(triple: &str) -> Self {
        Self {
            triple: triple.to_string(),
            cc: "gcc".to_string(),
            cxx: "g++".to_string(),
            ar: "ar".to_string(),
            ld: "ld".to_string(),
            strip: "strip".to_string(),
            objcopy: "objcopy".to_string(),
            pkg_config: "pkg-config".to_string(),
            native: true,
            cflags: Vec::new(),
            cxxflags: Vec::new(),
            ldflags: Vec::new(),
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
        assert!(!toolchain.native);
    }

    #[test]
    fn native_commands_keep_the_target_triple() {
        let toolchain = CrossToolchain::native("x86_64-linux-gnu");
        assert_eq!(toolchain.triple, "x86_64-linux-gnu");
        assert_eq!(toolchain.cc, "gcc");
        assert_eq!(toolchain.cxx, "g++");
        assert_eq!(toolchain.ar, "ar");
        assert_eq!(toolchain.strip, "strip");
        assert!(toolchain.native);
    }

    #[test]
    fn the_sysroot_is_a_fixed_guest_path() {
        assert!(SYSROOT.starts_with('/'));
        assert_ne!(SYSROOT, STAGE);
    }
}
