use std::{ffi::CString, io, path::Path};

#[allow(unused_variables)]
pub(super) fn cstr(p: &Path) -> io::Result<CString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(CString::new(p.as_os_str().as_bytes())?)
    }
}

// Convert Duration to Timespec
// It's strange that io_uring does not impl From<Duration> for Timespec.
pub(super) fn timespec(duration: std::time::Duration) -> io_uring::types::Timespec {
    io_uring::types::Timespec::new()
        .sec(duration.as_secs())
        .nsec(duration.subsec_nanos())
}

/// Do syscall and return Result<T, std::io::Error>
/// If use syscall@FD or syscall@NON_FD, the return value is wrapped in MaybeFd. The `MaybeFd` is
/// designed to close the fd when it is dropped.
/// If use syscall@RAW, the return value is raw value. The requirement to explicitly add @RAW is to
/// avoid misuse.
#[cfg(unix)]
#[macro_export]
macro_rules! syscall {
    ($fn: ident @FD ( $($arg: expr),* $(,)* ) ) => {{
        let res = unsafe { libc::$fn($($arg, )*) };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { $crate::driver::op::MaybeFd::new_fd(res as u32) })
        }
    }};
    ($fn: ident @NON_FD ( $($arg: expr),* $(,)* ) ) => {{
        let res = unsafe { libc::$fn($($arg, )*) };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok($crate::driver::op::MaybeFd::new_non_fd(res as u32))
        }
    }};
    ($fn: ident @RAW ( $($arg: expr),* $(,)* ) ) => {{
        let res = unsafe { libc::$fn($($arg, )*) };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}


