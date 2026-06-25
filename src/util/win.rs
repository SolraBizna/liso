//! This module contains utilities required for proper functioning on Windows.

use std::{os::windows::prelude::*, thread::JoinHandle};

use libc::{c_int, close, dup, dup2, pipe};
use windows::Win32::{
    Foundation::HANDLE,
    System::Threading::{
        QueueUserAPC2, QUEUE_USER_APC_FLAGS_SPECIAL_USER_APC,
    },
};

/// Wraps a JoinHandle on a thread that will be reading from stdin. Creates a
/// flimsy way for us to interrupt it, by taking away its stdin file descriptor
/// and sending it a "special APC". Very icky.
pub(crate) struct InterruptibleStdinThread {
    join_handle: Option<JoinHandle<()>>,
}

extern "system" fn dummy_handler(_: usize) {}

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
            let mut array: [c_int; 2] = [0; 2];
            let ret = pipe(&mut array[0], 32, 0);
            if ret != 0 {
                panic!("unable to create a body double for stdin");
            }
            let [rx, tx] = array;
            close(tx); // close the write side
            let hidden_stdin = dup(0);
            if hidden_stdin < 0 {
                panic!("unable to put stdin into witness relocation");
            }
            let replaced_stdin = dup2(rx, 0);
            if replaced_stdin < 0 {
                panic!("unable to replace stdin with a body double");
            }
            assert_eq!(
                replaced_stdin, 0,
                "attempt to replace stdin with a body double failed \
                despite appearing to succeed"
            );
            if (!QueueUserAPC2(
                Some(dummy_handler),
                HANDLE(join_handle.as_raw_handle()),
                0,
                QUEUE_USER_APC_FLAGS_SPECIAL_USER_APC,
            ))
            .into()
            {
                panic!("stdin thread did not take the bait");
            }
            join_handle.join().expect("unable to join stdin thread");
            let new_stdin = dup2(hidden_stdin, 0);
            if new_stdin < 0 {
                panic!("unable to restore stdin");
            }
            assert_eq!(
                new_stdin, 0,
                "attempt to restore stdin failed despite appearing to succeed"
            );
            close(hidden_stdin);
            close(rx);
        }
    }
    pub fn placebo_check() {
        // do nothing, as we are not a placebo
    }
}
