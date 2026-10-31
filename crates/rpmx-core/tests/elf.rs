use std::fs;

use rpmx_core::elf;

#[test]
fn the_test_binary_is_an_elf_with_libraries() {
    let data = fs::read(std::env::current_exe().expect("current exe")).expect("read exe");
    let info = elf::inspect(&data).expect("ELF");
    assert!(info.is_64 == (usize::BITS == 64));
    assert!(info.soname.is_none(), "an executable has no SONAME");
    assert!(
        info.needed.iter().any(|name| name.starts_with("libc.so")),
        "needed libraries: {:?}",
        info.needed
    );
    assert!(
        info.version_needs
            .iter()
            .any(|(library, version)| library.starts_with("libc.so") && !version.is_empty()),
        "version needs: {:?}",
        info.version_needs
    );
}

#[test]
fn non_elf_data_is_ignored() {
    assert!(elf::inspect(b"#!/bin/sh\necho hi\n").is_none());
}

#[test]
fn soname_dependency_uses_the_rpm_form() {
    assert_eq!(
        elf::soname_dependency("libzstd.so.1", true),
        "libzstd.so.1()(64bit)"
    );
    assert_eq!(
        elf::soname_dependency("libzstd.so.1", false),
        "libzstd.so.1()(32bit)"
    );
}

#[test]
fn pkgconfig_provide_reads_the_version() {
    let data = b"prefix=/usr\nName: demo\nVersion: 1.2.3\n";
    assert_eq!(
        elf::pkgconfig_provide("/usr/lib64/pkgconfig/demo.pc", data).as_deref(),
        Some("pkgconfig(demo) = 1.2.3")
    );
    assert_eq!(
        elf::pkgconfig_provide("/usr/lib64/pkgconfig/demo.pc", b"Name: demo\n").as_deref(),
        Some("pkgconfig(demo)")
    );
    assert!(elf::pkgconfig_provide("/usr/bin/demo", data).is_none());
}
