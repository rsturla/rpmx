use goblin::elf::Elf;

/// The parts of an ELF object that matter for RPM dependency generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElfInfo {
    pub is_64: bool,
    /// ELF `e_machine`, checked against the package target.
    pub machine: u16,
    /// `PT_INTERP`, the dynamic loader path the kernel runs.
    pub interpreter: Option<String>,
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
    let interpreter = elf.interpreter.map(str::to_string);
    Some(ElfInfo {
        is_64: elf.is_64,
        machine: elf.header.e_machine,
        interpreter,
        soname: elf.soname.map(str::to_string),
        needed: elf.libraries.into_iter().map(str::to_string).collect(),
        version_needs,
        version_defs,
    })
}

/// Read and inspect a file, returning `None` when it is not an ELF object.
pub fn inspect_file(path: &std::path::Path) -> Option<ElfInfo> {
    let data = std::fs::read(path).ok()?;
    inspect(&data)
}

/// Reject a staged ELF object built for the wrong architecture.
///
/// Without this a cross build that silently used the host compiler produces an
/// installable package full of host binaries, and nothing fails until the
/// package runs on the target.
pub fn require_machine(
    path: &str,
    info: &ElfInfo,
    target: rpmx_base::arch::Arch,
) -> Result<(), rpmx_base::error::Error> {
    if info.machine != target.elf_machine() {
        return Err(rpmx_base::error::Error::new(
            rpmx_base::error::codes::BUILD_008,
            format!(
                "{path} is an ELF object for machine {}, but the package target is {target} ({})",
                info.machine,
                target.elf_machine()
            ),
        ));
    }
    if let Some(interpreter) = &info.interpreter {
        if !interpreter.starts_with('/') {
            return Err(rpmx_base::error::Error::new(
                rpmx_base::error::codes::BUILD_008,
                format!("{path} has a relative PT_INTERP '{interpreter}'"),
            ));
        }
    }
    Ok(())
}

/// The RPM dependency form of a shared-object name.
pub fn soname_dependency(name: &str, is_64: bool) -> String {
    format!("{name}()({}bit)", if is_64 { "64" } else { "32" })
}

pub fn versioned_dependency(library: &str, version: &str, is_64: bool) -> String {
    format!(
        "{library}({version})({}bit)",
        if is_64 { "64" } else { "32" }
    )
}

/// Staged ELF objects whose `DT_NEEDED` cannot be found in the sysroot.
///
/// This is the closest thing to `ldd` for a target that cannot run. It is a
/// file-existence check rather than symbol resolution, but it catches the two
/// failures a cross build actually produces: a library the sysroot does not
/// contain, and a host library that was linked by mistake.
///
/// The sysroot is indexed by file name, so no distribution layout is assumed.
pub fn unresolved_needed(
    stage: &std::path::Path,
    sysroot: &std::path::Path,
) -> std::io::Result<Vec<(String, String)>> {
    let mut available = std::collections::BTreeSet::new();
    // A staged payload may carry private libraries alongside the objects that
    // need them, such as glibc's gconv modules. Those are real runtime
    // providers and must count before the root is consulted.
    index_names(stage, &mut available)?;
    index_names(sysroot, &mut available)?;
    let mut missing = Vec::new();
    let mut files = Vec::new();
    collect_files(stage, &mut files)?;
    for path in files {
        let Some(info) = inspect_file(&path) else {
            continue;
        };
        for needed in &info.needed {
            if !available.contains(needed.as_str()) {
                missing.push((path.display().to_string(), needed.clone()));
            }
        }
    }
    Ok(missing)
}

fn collect_files(
    directory: &std::path::Path,
    files: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn index_names(
    directory: &std::path::Path,
    names: &mut std::collections::BTreeSet<String>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            index_names(&path, names)?;
        } else if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            names.insert(name.to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_elf() -> std::path::PathBuf {
        std::fs::canonicalize(std::env::current_exe().expect("test binary")).expect("path")
    }

    fn other_arch() -> rpmx_base::arch::Arch {
        match rpmx_base::arch::Arch::current().expect("host") {
            rpmx_base::arch::Arch::X86_64 => rpmx_base::arch::Arch::Aarch64,
            _ => rpmx_base::arch::Arch::X86_64,
        }
    }

    #[test]
    fn a_payload_for_the_wrong_machine_is_rejected() {
        let path = host_elf();
        let info = inspect_file(&path).expect("elf");
        let host = rpmx_base::arch::Arch::current().expect("host");
        require_machine("usr/bin/demo", &info, host).expect("host machine matches");

        let error = require_machine("usr/bin/demo", &info, other_arch()).expect_err("wrong arch");
        assert_eq!(error.code, "RPMX-BUILD-008");
        assert!(error.message.contains("machine"), "{}", error.message);
    }

    #[test]
    fn a_staged_sibling_satisfies_a_needed_library() {
        let root = tempfile::tempdir().expect("work");
        let stage = root.path().join("stage");
        let sysroot = root.path().join("sysroot");
        std::fs::create_dir_all(stage.join("usr/bin")).expect("stage");
        std::fs::create_dir_all(stage.join("usr/lib64")).expect("stage lib");
        std::fs::create_dir_all(&sysroot).expect("sysroot");

        let binary = host_elf();
        std::fs::copy(&binary, stage.join("usr/bin/demo")).expect("copy");
        let info = inspect_file(&stage.join("usr/bin/demo")).expect("elf");
        for needed in &info.needed {
            std::fs::write(stage.join("usr/lib64").join(needed), b"").expect("lib");
        }
        assert!(
            unresolved_needed(&stage, &sysroot)
                .expect("scan")
                .is_empty(),
            "staged siblings are runtime providers"
        );
    }

    #[test]
    fn a_missing_soname_is_reported() {
        let root = tempfile::tempdir().expect("work");
        let stage = root.path().join("stage");
        let sysroot = root.path().join("sysroot");
        std::fs::create_dir_all(stage.join("usr/bin")).expect("stage");
        std::fs::create_dir_all(sysroot.join("usr/lib64")).expect("sysroot");

        let binary = host_elf();
        std::fs::copy(&binary, stage.join("usr/bin/demo")).expect("copy");
        let info = inspect_file(&stage.join("usr/bin/demo")).expect("elf");
        assert!(
            !info.needed.is_empty(),
            "the test binary is dynamically linked"
        );
        for needed in &info.needed {
            std::fs::write(sysroot.join("usr/lib64").join(needed), b"").expect("lib");
        }
        assert!(
            unresolved_needed(&stage, &sysroot)
                .expect("scan")
                .is_empty(),
            "every needed library is present"
        );

        let removed = info.needed[0].clone();
        std::fs::remove_file(sysroot.join("usr/lib64").join(&removed)).expect("remove");
        let missing = unresolved_needed(&stage, &sysroot).expect("scan");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].1, removed);
    }
}
