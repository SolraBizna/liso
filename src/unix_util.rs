//! This module contains utility functions required for proper functioning on
//! UNIX.

pub(crate) fn sigstop_ourselves() {
    unsafe {
        libc::raise(libc::SIGSTOP);
    }
}
