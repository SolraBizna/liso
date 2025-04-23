//! This module contains utilites required for proper functioning on UNIX.

use std::{
    os::{fd::AsRawFd, unix::thread::JoinHandleExt},
    thread::JoinHandle,
};

use nix::{
    sys::{
        pthread::pthread_kill,
        signal::{
            raise, sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal,
        },
    },
    unistd::{close, dup, dup2, pipe},
};

pub(crate) fn sigstop_ourselves() {
    let _ = raise(Signal::SIGSTOP);
}

/// Wraps a JoinHandle on a thread that will be reading from stdin. Creates a
/// flimsy way for us to interrupt it, by taking away its stdin file descriptor
/// and sending it a signal. Very icky.
pub(crate) struct InterruptibleStdinThread {
    join_handle: Option<JoinHandle<()>>,
}

extern "C" fn dummy_handler(_: i32) {}

impl InterruptibleStdinThread {
    pub fn new(join_handle: JoinHandle<()>) -> InterruptibleStdinThread {
        InterruptibleStdinThread {
            join_handle: Some(join_handle),
        }
    }
    pub fn interrupt(&mut self) {
        let Some(join_handle) = self.join_handle.take() else {
            return;
        };
        if join_handle.is_finished() {
            return;
        }
        // oh boy!
        unsafe {
            let (rx, tx) =
                pipe().expect("unable to create a body double for stdin");
            // note: pipe returns OwnedFds, so rx and tx will close on drop
            drop(tx); // close the write side
            let hidden_stdin =
                dup(0).expect("unable to put stdin into witness relocation");
            let new_action = SigAction::new(
                SigHandler::Handler(dummy_handler),
                SaFlags::empty(),
                SigSet::empty(),
            );
            let old_action = sigaction(Signal::SIGHUP, &new_action)
                .expect("unable to override SIGHUP handler");
            let replaced_stdin = dup2(rx.as_raw_fd(), 0)
                .expect("unable to replace stdin with a body double");
            assert_eq!(
                replaced_stdin, 0,
                "attempt to replace stdin with a body double failed \
                despite appearing to succeed"
            );
            let _ =
                pthread_kill(join_handle.as_pthread_t(), Some(Signal::SIGHUP));
            join_handle.join().expect("unable to join stdin thread");
            sigaction(Signal::SIGHUP, &old_action)
                .expect("unable to restore SIGHUP handler");
            let new_stdin =
                dup2(hidden_stdin, 0).expect("unable to restore stdin");
            assert_eq!(
                new_stdin, 0,
                "attempt to restore stdin failed despite appearing to succeed"
            );
            let _ = close(hidden_stdin);
        }
    }
    pub fn placebo_check() {
        // do nothing, as we are not a placebo
    }
}
