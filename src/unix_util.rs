//! This module contains utility functions required for proper functioning on
//! UNIX.

use std::{os::unix::thread::JoinHandleExt, thread::JoinHandle};

use nix::{
    sys::{
        pthread::pthread_kill,
        signal::{
            raise, sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal,
        },
    },
    unistd::{close, dup, dup2},
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
            let hidden_stdin =
                dup(0).expect("unable to put stdin into witness relocation");
            let new_action = SigAction::new(
                SigHandler::Handler(dummy_handler),
                SaFlags::empty(),
                SigSet::empty(),
            );
            let old_action = sigaction(Signal::SIGHUP, &new_action)
                .expect("unable to override SIGHUP handler");
            close(0).expect("unable to fake stdin's death");
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
}
