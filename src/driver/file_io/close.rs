use std::io;
use io_uring::{opcode, types};
use io_uring::squeue::Entry;
use libc::c_int;
use crate::driver::op::{Op, Mappable};

pub(crate) struct Close {
    fd: c_int,
}

impl Op<Close> {
    pub(crate) fn close(fd: c_int) -> io::Result<Op<Close>> {
        Op::try_submit_with(Close { fd })
    }
}

impl Mappable for Close {
    const SKIP_CANCEL: bool = true;

    fn uring_op(&mut self) -> Entry {
        opcode::Close::new(types::Fd(self.fd)).build()
    }

}