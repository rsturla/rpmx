use std::fmt;
use std::str::FromStr;

use crate::error::{codes, Error, Result};

/// A normalized RPM architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Arch {
    X86_64,
    Aarch64,
    Ppc64le,
    S390x,
    I686,
}

impl Arch {
    pub const ALL: &'static [Self] = &[
        Self::X86_64,
        Self::Aarch64,
        Self::Ppc64le,
        Self::S390x,
        Self::I686,
    ];

    /// The architecture this process is running on.
    pub fn current() -> Result<Self> {
        Self::parse(std::env::consts::ARCH)
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "x86_64" | "amd64" => Ok(Self::X86_64),
            "aarch64" | "arm64" => Ok(Self::Aarch64),
            "ppc64le" | "powerpc64le" | "powerpc64" => Ok(Self::Ppc64le),
            "s390x" => Ok(Self::S390x),
            "i686" | "i386" | "x86" => Ok(Self::I686),
            other => Err(Error::new(
                codes::RUNNER_001,
                format!("unsupported architecture '{other}'"),
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Ppc64le => "ppc64le",
            Self::S390x => "s390x",
            Self::I686 => "i686",
        }
    }

    pub const fn is_64(self) -> bool {
        !matches!(self, Self::I686)
    }

    /// The architecture name used by OCI manifest indexes.
    pub const fn oci_arch(self) -> &'static str {
        match self {
            Self::X86_64 => "amd64",
            Self::Aarch64 => "arm64",
            Self::Ppc64le => "ppc64le",
            Self::S390x => "s390x",
            Self::I686 => "386",
        }
    }

    /// The GNU target triple a cross toolchain is named after.
    ///
    /// This is the compiler's name for the architecture, not the RPM name:
    /// `ppc64le` is `powerpc64le` to GCC, and Fedora ships the toolchain as
    /// `gcc-<triple>`. Keep this in step with [`Arch::as_str`]; a wrong triple
    /// silently selects a host compiler instead of failing.
    pub const fn triple(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64-linux-gnu",
            Self::Aarch64 => "aarch64-linux-gnu",
            Self::Ppc64le => "powerpc64le-linux-gnu",
            Self::S390x => "s390x-linux-gnu",
            Self::I686 => "i686-linux-gnu",
        }
    }

    /// The ELF `e_machine` value for this architecture.
    ///
    /// Used to reject a payload that does not match the package's target
    /// architecture, which is the only way a cross build can silently produce
    /// a package containing host binaries.
    pub const fn elf_machine(self) -> u16 {
        match self {
            // EM_X86_64
            Self::X86_64 => 62,
            // EM_AARCH64
            Self::Aarch64 => 183,
            // EM_PPC64
            Self::Ppc64le => 21,
            // EM_S390
            Self::S390x => 22,
            // EM_386
            Self::I686 => 3,
        }
    }
}

/// The native architecture as an RPM string. Used as the CLI default.
pub fn native_arch() -> &'static str {
    Arch::current().map(Arch::as_str).unwrap_or("x86_64")
}

impl fmt::Display for Arch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Arch {
    type Err = Error;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_normalize() {
        assert_eq!(Arch::parse("amd64").unwrap(), Arch::X86_64);
        assert_eq!(Arch::parse("arm64").unwrap(), Arch::Aarch64);
        assert_eq!(Arch::parse("powerpc64le").unwrap(), Arch::Ppc64le);
        assert_eq!(Arch::parse("x86").unwrap(), Arch::I686);
    }

    #[test]
    fn unknown_architectures_are_rejected() {
        let error = Arch::parse("sparc").expect_err("unsupported arch");
        assert_eq!(error.code, "RPMX-RUNNER-001");
    }

    #[test]
    fn every_rpm_arch_has_an_oci_name() {
        for (arch, oci) in [
            (Arch::X86_64, "amd64"),
            (Arch::Aarch64, "arm64"),
            (Arch::Ppc64le, "ppc64le"),
            (Arch::S390x, "s390x"),
            (Arch::I686, "386"),
        ] {
            assert_eq!(arch.oci_arch(), oci);
        }
    }

    #[test]
    fn every_rpm_arch_has_a_distinct_triple_and_elf_machine() {
        let mut triples = std::collections::BTreeSet::new();
        let mut machines = std::collections::BTreeSet::new();
        for arch in [
            Arch::X86_64,
            Arch::Aarch64,
            Arch::Ppc64le,
            Arch::S390x,
            Arch::I686,
        ] {
            assert!(
                arch.triple().starts_with(arch.as_str()) || arch == Arch::Ppc64le,
                "{} has triple {}",
                arch,
                arch.triple()
            );
            assert!(triples.insert(arch.triple()), "duplicate triple for {arch}");
            assert!(
                machines.insert(arch.elf_machine()),
                "duplicate ELF machine for {arch}"
            );
        }
        assert_eq!(Arch::Aarch64.triple(), "aarch64-linux-gnu");
        assert_eq!(Arch::Ppc64le.triple(), "powerpc64le-linux-gnu");
        assert_eq!(Arch::Aarch64.elf_machine(), 183);
        assert_eq!(Arch::X86_64.elf_machine(), 62);
    }
}
