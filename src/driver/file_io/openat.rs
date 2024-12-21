use std::ffi::CString;
use std::io;
use std::path::Path;
use io_uring::{opcode, types};
use crate::driver::op::{Op, Mappable};
use crate::driver::util::cstr;

pub(crate) struct OpenAt {
    pub(crate) fd: i32,
    pub(crate) path: CString,
    pub(crate) flags: i32,
    pub(crate) mode: libc::mode_t,
}

impl Op<OpenAt> {
    pub(crate) fn openat<P: AsRef<Path>>(
        dir_fd: i32,
        path: P,
        flags: i32,
        mode: libc::mode_t,
    ) -> io::Result<Op<OpenAt>> {
        let path = cstr(path.as_ref())?;
        let open_at = OpenAt {
            fd: dir_fd,
            path,
            flags,
            mode,
        };

        Op::submit_with(open_at)
    }
}

impl Mappable for OpenAt {
    const RET_IS_FD: bool = true;
    fn uring_op(&mut self) -> io_uring::squeue::Entry {
        opcode::OpenAt::new(types::Fd(self.fd), self.path.as_c_str().as_ptr())
            .flags(self.flags)
            .mode(self.mode)
            .build()
    }
}

