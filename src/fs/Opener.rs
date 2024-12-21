use crate::driver::op::Op;
use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, RawFd};
use std::path::Path;

pub(crate) struct Opener {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    pub(crate) mode: libc::mode_t,
}
impl Opener {
    pub fn new() -> Self {
        Opener {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            mode: 0o666,
        }
    }
    pub fn read(&mut self, read: bool) -> &mut Opener {
        self.read = read;
        self
    }

    pub fn write(&mut self, write: bool) -> &mut Opener {
        self.write = write;
        self
    }

    pub fn append(&mut self, append: bool) -> &mut Opener {
        self.append = append;
        self
    }

    pub fn truncate(&mut self, truncate: bool) -> &mut Opener {
        self.truncate = truncate;
        self
    }

    pub fn create(&mut self, create: bool) -> &mut Opener {
        self.create = create;
        self
    }

    pub fn create_new(&mut self, create_new: bool) -> &mut Opener {
        self.create_new = create_new;
        self
    }

    pub async unsafe fn openat(&self, dir_fd: i32, path: impl AsRef<Path>) -> io::Result<File> {
        let op = Op::openat(
            dir_fd,
            path.as_ref(),
            self.access_mode()? | self.creation_mode()?,
            self.mode,
        )?;

        let completion = op.await;
        //TODO
        //Note: The fd needs to be closed manually
        //automatic submission of the close operation to io_uring is not yet implemented.
        Ok(File::from_raw_fd(RawFd::from(completion.meta.result?.into_inner() as i32)))
    }
    pub(crate) fn access_mode(&self) -> io::Result<libc::c_int> {
        match (self.read, self.write, self.append) {
            (true, false, false) => Ok(libc::O_RDONLY),
            (false, true, false) => Ok(libc::O_WRONLY),
            (true, true, false) => Ok(libc::O_RDWR),
            (false, _, true) => Ok(libc::O_WRONLY | libc::O_APPEND),
            (true, _, true) => Ok(libc::O_RDWR | libc::O_APPEND),
            (false, false, false) => Err(io::Error::from_raw_os_error(libc::EINVAL)),
        }
    }
    pub(crate) fn creation_mode(&self) -> io::Result<libc::c_int> {
        match (self.write, self.append) {
            (true, false) => {}
            (false, false) => {
                if self.truncate || self.create || self.create_new {
                    return Err(io::Error::from_raw_os_error(libc::EINVAL));
                }
            }
            (_, true) => {
                if self.truncate && !self.create_new {
                    return Err(io::Error::from_raw_os_error(libc::EINVAL));
                }
            }
        }

        Ok(match (self.create, self.truncate, self.create_new) {
            (false, false, false) => 0,
            (true, false, false) => libc::O_CREAT,
            (false, true, false) => libc::O_TRUNC,
            (true, true, false) => libc::O_CREAT | libc::O_TRUNC,
            (_, _, true) => libc::O_CREAT | libc::O_EXCL,
        })
    }
}
