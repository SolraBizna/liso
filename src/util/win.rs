//! This module contains utilities required for proper functioning on Windows.

use std::{
    mem::zeroed,
    sync::{
        atomic::{AtomicBool, Ordering},
        LazyLock,
    },
    thread::JoinHandle,
};

use libc::isatty;
use windows::{
    core::BOOL,
    Win32::{
        System::Console::{
            FlushConsoleInputBuffer, GetConsoleMode,
            GetConsoleScreenBufferInfo, GetStdHandle, SetConsoleMode,
            WriteConsoleInputW, BACKGROUND_BLUE, BACKGROUND_GREEN,
            BACKGROUND_RED, CONSOLE_CHARACTER_ATTRIBUTES, CONSOLE_MODE,
            ENABLE_LVB_GRID_WORLDWIDE, ENABLE_PROCESSED_OUTPUT,
            ENABLE_QUICK_EDIT_MODE, ENABLE_VIRTUAL_TERMINAL_INPUT,
            ENABLE_VIRTUAL_TERMINAL_PROCESSING, ENABLE_WINDOW_INPUT,
            FOREGROUND_BLUE, FOREGROUND_GREEN, FOREGROUND_RED, INPUT_RECORD,
            INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0,
            STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        },
        UI::Input::KeyboardAndMouse::VK_RETURN,
    },
};

static IS_RAW: AtomicBool = AtomicBool::new(false);

static STDIN_BEING_INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Wraps a JoinHandle on a thread that will be reading from stdin. Creates a
/// flimsy way for us to interrupt it, by taking away its stdin file descriptor
/// and sending it a "special APC". Very icky.
pub(crate) struct InterruptibleStdinThread {
    join_handle: Option<JoinHandle<()>>,
}

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
        unsafe {
            STDIN_BEING_INTERRUPTED.store(true, Ordering::SeqCst);
            // We can't use the fd-0 switcheroo we use on UNIX to do this on
            // Windows, because Windows Rust uses NtReadFile to read the stdin
            // handle, not libc. Fortunately, we have the option to simulate
            // input instead.
            let buf = [
                INPUT_RECORD {
                    EventType: KEY_EVENT as u16,
                    Event: INPUT_RECORD_0 {
                        KeyEvent: KEY_EVENT_RECORD {
                            bKeyDown: BOOL(1),
                            wRepeatCount: 1,
                            wVirtualKeyCode: VK_RETURN.0,
                            wVirtualScanCode: 0x0A,
                            uChar: KEY_EVENT_RECORD_0 { UnicodeChar: 0x1B },
                            dwControlKeyState: 0,
                        },
                    },
                },
                INPUT_RECORD {
                    EventType: KEY_EVENT as u16,
                    Event: INPUT_RECORD_0 {
                        KeyEvent: KEY_EVENT_RECORD {
                            bKeyDown: BOOL(0),
                            wRepeatCount: 0,
                            wVirtualKeyCode: VK_RETURN.0,
                            wVirtualScanCode: 0x0A,
                            uChar: KEY_EVENT_RECORD_0 { UnicodeChar: 0x1B },
                            dwControlKeyState: 0,
                        },
                    },
                },
            ];
            let Ok(stdin_handle) = GetStdHandle(STD_INPUT_HANDLE) else {
                return;
            };
            loop {
                // This loop SHOULD eventually terminate...
                let _ = FlushConsoleInputBuffer(stdin_handle);
                let mut wrote = 0;
                let _ = WriteConsoleInputW(stdin_handle, &buf, &raw mut wrote);
                if wrote == 2 {
                    break;
                } else {
                    std::thread::yield_now();
                }
            }
            join_handle.join().expect("unable to join stdin thread");
            STDIN_BEING_INTERRUPTED.store(false, Ordering::SeqCst);
            // We can't be sure that Rust has consumed all the input. Or any of
            // it. Make sure none is left over for the next reader with one
            // last flush.
            let _ = FlushConsoleInputBuffer(stdin_handle);
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
pub fn enter_raw_mode(ansi: bool) -> bool {
    unsafe {
        let Ok(stdin_handle) = GetStdHandle(STD_INPUT_HANDLE) else {
            return false;
        };
        let Ok(stdout_handle) = GetStdHandle(STD_OUTPUT_HANDLE) else {
            return false;
        };
        loop {
            match IS_RAW.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
                Ok(_) => break,
                Err(true) => panic!("BUG IN LISO: enter_raw_mode() called twice without exit_raw_mode() in between!"),
                Err(false) => continue,
            }
        }
        let Ok(_) = GetConsoleMode(stdin_handle, &raw mut OLD_STDIN_MODE)
        else {
            IS_RAW.store(false, Ordering::Release);
            return false;
        };
        let Ok(_) = GetConsoleMode(stdout_handle, &raw mut OLD_STDOUT_MODE)
        else {
            IS_RAW.store(false, Ordering::Release);
            return false;
        };
        let mut in_mode = ENABLE_QUICK_EDIT_MODE | ENABLE_WINDOW_INPUT;
        if ansi {
            in_mode |= ENABLE_VIRTUAL_TERMINAL_INPUT;
        }
        let mut out_mode = ENABLE_PROCESSED_OUTPUT | ENABLE_LVB_GRID_WORLDWIDE;
        if ansi {
            out_mode |= ENABLE_VIRTUAL_TERMINAL_PROCESSING;
        }
        let Ok(_) = SetConsoleMode(stdin_handle, in_mode) else {
            IS_RAW.store(false, Ordering::Release);
            return false;
        };
        let Ok(_) = SetConsoleMode(stdout_handle, out_mode) else {
            panic!("we were able to set the console input mode, but not the console output mode");
        };
        true
    }
}

/// Restores the previous terminal mode, whatever that was. Guaranteed to only
/// be called if enter_raw_mode() has previously succeeded. Might be called
/// twice if we panic.
pub fn exit_raw_mode() {
    if !IS_RAW.load(Ordering::Relaxed) {
        return;
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

pub fn stdin_being_interrupted() -> bool {
    STDIN_BEING_INTERRUPTED.load(Ordering::Relaxed)
}

pub struct WindowsInfo {
    pub ansi_support: bool,
    pub default_fg: CONSOLE_CHARACTER_ATTRIBUTES,
    pub default_bg: CONSOLE_CHARACTER_ATTRIBUTES,
}

fn get_windows_info() -> Result<WindowsInfo, crate::DummyError> {
    unsafe {
        let stdout_handle = GetStdHandle(STD_OUTPUT_HANDLE)?;
        let mut ansi_support = false;
        if std::env::var("LISO_FORCE_WINDOWS_CONSOLE").is_err() {
            let mut mode = zeroed();
            GetConsoleMode(stdout_handle, &raw mut mode)?;
            ansi_support = mode.contains(ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            if !ansi_support {
                // maybe it has ANSI support that wasn't already enabled?
                let test_mode = mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
                if SetConsoleMode(stdout_handle, test_mode).is_ok() {
                    SetConsoleMode(stdout_handle, mode).unwrap();
                    ansi_support = true;
                }
            }
        }
        let mut csbi = zeroed();
        GetConsoleScreenBufferInfo(stdout_handle, &raw mut csbi)?;
        let default_fg = csbi.wAttributes
            & (FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_BLUE);
        let default_bg = csbi.wAttributes
            & (BACKGROUND_RED | BACKGROUND_GREEN | BACKGROUND_BLUE);
        Ok(WindowsInfo {
            ansi_support,
            default_fg,
            default_bg,
        })
    }
}

pub static WINDOWS_INFO: LazyLock<WindowsInfo> = LazyLock::new(|| {
    get_windows_info().unwrap_or_else(|_| WindowsInfo {
        ansi_support: false,
        default_fg: FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_BLUE,
        default_bg: CONSOLE_CHARACTER_ATTRIBUTES(0),
    })
});
