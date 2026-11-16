use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, RawFd};

pub(crate) fn adopt_handoff_file(fd: RawFd) -> io::Result<File> {
    let received = unsafe { File::from_raw_fd(fd) };
    received.try_clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, IntoRawFd};

    #[test]
    fn adopted_handoff_file_does_not_escape_to_children() {
        let source = File::open(std::env::current_exe().unwrap()).unwrap();
        let adopted = adopt_handoff_file(source.into_raw_fd()).unwrap();
        let flags = unsafe { libc::fcntl(adopted.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }
}
