//! This module contains utilities required for proper functioning on Windows.

use std::{os::windows::prelude::*, thread::JoinHandle};

use libc::{c_int, close, dup, dup2, isatty, pipe};
use windows::Win32::{
    Foundation::HANDLE,
    System::{
        Console::{
            GetConsoleMode, GetStdHandle, SetConsoleMode, CONSOLE_MODE,
            ENABLE_LVB_GRID_WORLDWIDE, ENABLE_PROCESSED_OUTPUT,
            ENABLE_QUICK_EDIT_MODE, ENABLE_WINDOW_INPUT, STD_INPUT_HANDLE,
            STD_OUTPUT_HANDLE,
        },
        Threading::{QueueUserAPC2, QUEUE_USER_APC_FLAGS_SPECIAL_USER_APC},
    },
};

#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(debug_assertions)]
static IS_RAW: AtomicBool = AtomicBool::new(false);

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

static mut OLD_STDIN_MODE: CONSOLE_MODE = CONSOLE_MODE(0);
static mut OLD_STDOUT_MODE: CONSOLE_MODE = CONSOLE_MODE(0);

/// Puts the terminal into raw mode. We can assume we will not be called twice
/// without raw mode being disabled in between. Return true if the input is a
/// tty and raw input is possible.
pub fn enter_raw_mode() -> bool {
    unsafe {
        let Ok(stdin_handle) = GetStdHandle(STD_INPUT_HANDLE) else {
            return false;
        };
        let Ok(stdout_handle) = GetStdHandle(STD_OUTPUT_HANDLE) else {
            return false;
        };
        #[cfg(debug_assertions)]
        loop {
            match IS_RAW.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => break,
                Err(true) => panic!("BUG IN LISO: enter_raw_mode() called twice without exit_raw_mode() in between!"),
                Err(false) => continue,
            }
        }
        let Ok(_) = GetConsoleMode(stdin_handle, &raw mut OLD_STDIN_MODE)
        else {
            #[cfg(debug_assertions)]
            IS_RAW.store(false, Ordering::Release);
            return false;
        };
        let Ok(_) = GetConsoleMode(stdout_handle, &raw mut OLD_STDOUT_MODE)
        else {
            #[cfg(debug_assertions)]
            IS_RAW.store(false, Ordering::Release);
            return false;
        };
        let Ok(_) = SetConsoleMode(
            stdin_handle,
            ENABLE_QUICK_EDIT_MODE | ENABLE_WINDOW_INPUT,
        ) else {
            #[cfg(debug_assertions)]
            IS_RAW.store(false, Ordering::Release);
            return false;
        };
        let Ok(_) = SetConsoleMode(
            stdout_handle,
            ENABLE_PROCESSED_OUTPUT | ENABLE_LVB_GRID_WORLDWIDE,
        ) else {
            panic!("we were able to set the console input mode, but not the console output mode");
        };
        true
    }
}

/// Restores the previous terminal mode, whatever that was. Guaranteed to only
/// be called if enter_raw_mode() has previously succeeded.
pub fn exit_raw_mode() {
    #[cfg(debug_assertions)]
    if !IS_RAW.load(Ordering::Relaxed) {
        panic!("BUG IN LISO: exit_raw_mode() called without preceding enter_raw_mode()!")
    } else {
        IS_RAW.store(false, Ordering::Release);
    }
    unsafe {
        let stdin_handle = GetStdHandle(STD_INPUT_HANDLE).unwrap();
        let stdout_handle = GetStdHandle(STD_OUTPUT_HANDLE).unwrap();
        let _ = SetConsoleMode(stdin_handle, OLD_STDIN_MODE);
        let _ = SetConsoleMode(stdout_handle, OLD_STDOUT_MODE);
    }
}

pub fn stdin_and_stdout_are_tty() -> bool {
    unsafe { isatty(0) != 0 && isatty(1) != 0 }
}
