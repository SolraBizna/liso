//! This module contains utility functions required for proper functioning on
//! UNIX.

use nix::sys::signal::{raise, Signal};

pub(crate) fn sigstop_ourselves() {
    let _ = raise(Signal::SIGSTOP);
}