use goblin::elf::Elf;

/// The parts of an ELF object that matter for RPM dependency generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElfInfo {
    pub is_64: bool,
    pub soname: Option<String>,
    pub needed: Vec<String>,
    /// `(library, version)` pairs from `.gnu.version_r`.
    pub version_needs: Vec<(String, String)>,
    /// Version names defined by `.gnu.version_d`.
    pub version_defs: Vec<String>,
}

/// Inspect a staged file. Returns `None` for anything that is not a parseable
/// ELF object; a static archive or a script is not an error.
pub fn inspect(data: &[u8]) -> Option<ElfInfo> {
    if !data.starts_with(b"\x7fELF") {
        return None;
    }
    let elf = Elf::parse(data).ok()?;
    let mut version_needs = Vec::new();
    if let Some(verneed) = &elf.verneed {
        for need in verneed.iter() {
            let Some(library) = elf.dynstrtab.get_at(need.vn_file) else {
                continue;
            };
            for auxiliary in need.iter() {
                if let Some(version) = elf.dynstrtab.get_at(auxiliary.vna_name) {
                    version_needs.push((library.to_string(), version.to_string()));
                }
            }
        }
    }
    let mut version_defs = Vec::new();
    if let Some(verdef) = &elf.verdef {
        for definition in verdef.iter() {
            for auxiliary in definition.iter() {
                if let Some(name) = elf.dynstrtab.get_at(auxiliary.vda_name) {
                    version_defs.push(name.to_string());
                }
            }
        }
    }
    Some(ElfInfo {
        is_64: elf.is_64,
        soname: elf.soname.map(str::to_string),
        needed: elf.libraries.into_iter().map(str::to_string).collect(),
        version_needs,
        version_defs,
    })
}

/// The RPM dependency form of a shared-object name.
///
/// Symbol versions (`libc.so.6(GLIBC_2.38)(64bit)`) are not generated yet;
/// the soname form is enough for dependency resolution.
pub fn soname_dependency(name: &str, is_64: bool) -> String {
    format!("{name}()({}bit)", if is_64 { "64" } else { "32" })
}

pub fn versioned_dependency(library: &str, version: &str, is_64: bool) -> String {
    format!(
        "{library}({version})({}bit)",
        if is_64 { "64" } else { "32" }
    )
}

/// `pkgconfig(foo)` and, when the file declares one, `pkgconfig(foo) = 1.2`.
pub fn pkgconfig_provide(path: &str, data: &[u8]) -> Option<String> {
    let file = path.rsplit('/').next()?;
    let name = file.strip_suffix(".pc")?;
    let text = std::str::from_utf8(data).ok()?;
    let version = text.lines().find_map(|line| {
        line.strip_prefix("Version:")
            .map(str::trim)
            .filter(|version| !version.is_empty())
    });
    Some(match version {
        Some(version) => format!("pkgconfig({name}) = {version}"),
        None => format!("pkgconfig({name})"),
    })
}
