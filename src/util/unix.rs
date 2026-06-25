//! This module contains utilites required for proper functioning on UNIX.

#[cfg(debug_assertions)]
use std::sync::atomic::AtomicBool;
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
        termios::{
            cfmakeraw, tcgetattr, tcsetattr, SetArg::TCSAFLUSH, Termios,
        },
    },
    unistd::{close, dup, dup2, isatty, pipe},
};

#[cfg(debug_assertions)]
use std::sync::atomic::Ordering;

#[cfg(debug_assertions)]
static IS_RAW: AtomicBool = AtomicBool::new(false);

static mut OLD_TERMIOS: Termios = unsafe { std::mem::zeroed() };

pub fn sigstop_ourselves() {
    let _ = raise(Signal::SIGSTOP);
}

/// Wraps a JoinHandle on a thread that will be reading from stdin. Creates a
/// flimsy way for us to interrupt it, by taking away its stdin file descriptor
/// and sending it a signal. Very icky.
pub struct InterruptibleStdinThread {
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

/// Puts the terminal into raw mode. We can assume we will not be called twice
/// without raw mode being disabled in between. Return true if the input is a
/// tty and raw input is possible.
pub fn enter_raw_mode() -> bool {
    if isatty(0) != Ok(true) || isatty(1) != Ok(true) {
        return false;
    }
    #[cfg(debug_assertions)]
    loop {
        match IS_RAW.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => break,
            Err(true) => panic!("BUG IN LISO: enter_raw_mode() called twice without exit_raw_mode() in between!"),
            Err(false) => continue,
        }
    }
    let stdin = std::io::stdin();
    let Ok(mut termios) = tcgetattr(&stdin) else {
        #[cfg(debug_assertions)]
        IS_RAW.store(false, Ordering::Release);
        return false;
    };
    unsafe {
        OLD_TERMIOS = termios.clone();
    }
    // TODO: not necessarily portable, consider alternatives
    cfmakeraw(&mut termios);
    let Ok(_) = tcsetattr(&stdin, TCSAFLUSH, &termios) else {
        #[cfg(debug_assertions)]
        IS_RAW.store(false, Ordering::Release);
        return false;
    };
    true
}

/// Restores the previous terminal mode, whatever that was. Guaranteed to only
/// be called if enter_raw_mode() has previously succeeded.
pub fn exit_raw_mode() {
    #[cfg(debug_assertions)]
    if !IS_RAW.load(Ordering::Relaxed) {
        panic!("BUG IN LISO: exit_raw_mode() called without preceding enter_raw_mode()!")
    }
    // this can easily fail if the terminal has gone away, in which case,
    // failure of this step is not harmful
    #[allow(static_mut_refs)]
    let _ = tcsetattr(std::io::stdin(), TCSAFLUSH, unsafe { &OLD_TERMIOS });
    #[cfg(debug_assertions)]
    loop {
        match IS_RAW.compare_exchange(
            true,
            false,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(false) => panic!(
                "two threads raced to turn off raw mode, and we lost :("
            ),
            Err(true) => continue,
        }
    }
}

pub fn stdin_and_stdout_are_tty() -> bool {
    isatty(0) == Ok(true) && isatty(1) == Ok(true)
}
