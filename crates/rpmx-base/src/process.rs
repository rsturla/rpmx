use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

/// Spawn with only standard streams and explicitly borrowed descriptors inherited.
/// Parent descriptor flags remain unchanged, including during concurrent startup.
pub fn spawn_sealed(
    mut command: Command,
    descriptors: &[BorrowedFd<'_>],
) -> std::io::Result<Child> {
    // No descriptor can have this unsigned number (FDs are signed ints).
    // Probe syscall/flag availability without touching the parent's table,
    // so later spawn/exec errors retain their original meaning.
    let available = unsafe {
        libc::syscall(
            libc::SYS_close_range,
            u32::MAX,
            u32::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        )
    };
    if available < 0 {
        let error = std::io::Error::last_os_error();
        let kind = if matches!(
            error.raw_os_error(),
            Some(libc::ENOSYS) | Some(libc::EINVAL)
        ) {
            std::io::ErrorKind::Unsupported
        } else {
            error.kind()
        };
        return Err(std::io::Error::new(kind, format!(
            "descriptor isolation requires Linux 5.11+ with close_range(CLOSE_RANGE_CLOEXEC) permitted by host policy: {error}"
        )));
    }
    let descriptors: Vec<_> = descriptors.iter().map(AsRawFd::as_raw_fd).collect();
    // SAFETY: the child hook uses only close_range/fcntl syscalls and errno
    // access. Marking descriptors instead of closing them preserves Rust's
    // CLOEXEC error pipe until exec can report any startup failure.
    unsafe {
        command.pre_exec(move || {
            if libc::syscall(
                libc::SYS_close_range,
                3_u32,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            ) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            for descriptor in &descriptors {
                let flags = libc::fcntl(*descriptor, libc::F_GETFD);
                if flags < 0
                    || libc::fcntl(*descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    // Spawn while every borrowed descriptor is still alive, and consume the
    // Command so its raw descriptor hook cannot be reused after this call.
    command.spawn()
}
