// At the time of this writing (2026-07-07), the documentation used to write
// this module was present at the following URL:
//
// <https://learn.microsoft.com/en-us/windows/console/>

use windows::{
    core::BOOL,
    Win32::{
        Foundation::HANDLE,
        System::Console::{
            FillConsoleOutputAttribute, FillConsoleOutputCharacterW,
            GetConsoleCursorInfo, GetConsoleScreenBufferInfo, GetStdHandle,
            ReadConsoleInputW, ScrollConsoleScreenBufferW,
            SetConsoleCursorInfo, SetConsoleCursorPosition,
            SetConsoleTextAttribute, WriteConsoleInputW, WriteConsoleW,
            BACKGROUND_BLUE, BACKGROUND_GREEN, BACKGROUND_RED, CHAR_INFO,
            CHAR_INFO_0, COMMON_LVB_UNDERSCORE, CONSOLE_CHARACTER_ATTRIBUTES,
            COORD, FOREGROUND_BLUE, FOREGROUND_GREEN, FOREGROUND_INTENSITY,
            FOREGROUND_RED, INPUT_RECORD, KEY_EVENT, SMALL_RECT,
            STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, WINDOW_BUFFER_SIZE_EVENT,
        },
        UI::Input::KeyboardAndMouse::{
            VIRTUAL_KEY, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE,
            VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_TAB,
            VK_UP,
        },
    },
};

use crate::util::WINDOWS_INFO;

use super::*;

use std::{mem::zeroed, panic};

use super::KeyCode;

const USER_INTERRUPT_EVENT: u32 = 0x4000;

/// Uses the Windows Console API for input and output.
pub(crate) struct WinTerminal {
    suspended: bool,
    old_hook:
        Option<Box<dyn Fn(&panic::PanicHookInfo<'_>) + Sync + Send + 'static>>,
    stdin_handle: HANDLE,
    stdout_handle: HANDLE,
    cur_attributes: CONSOLE_CHARACTER_ATTRIBUTES,
    cur_style: Style,
}

fn input_thread(
    stdin_handle: HANDLE,
    req_tx: std_mpsc::Sender<Request>,
) -> LifeOrDeath {
    unsafe {
        let mut high_surrogate: Option<u16> = None;
        let mut buf: [INPUT_RECORD; 5] = zeroed();
        let mut red;
        loop {
            red = 0;
            ReadConsoleInputW(stdin_handle, &mut buf, &raw mut red)?;
            for event in buf[..red as usize].iter() {
                match event.EventType as u32 {
                    USER_INTERRUPT_EVENT => {
                        return Ok(());
                    }
                    KEY_EVENT => {
                        let event = &event.Event.KeyEvent;
                        if !event.bKeyDown.as_bool() {
                            continue;
                        }
                        match VIRTUAL_KEY(event.wVirtualKeyCode) {
                            VK_BACK => req_tx
                                .send(Request::Key(KeyCode::Backspace))?,
                            VK_TAB => req_tx.send(Request::Char('\t'))?,
                            VK_RETURN => req_tx.send(Request::Char('\n'))?,
                            VK_ESCAPE => req_tx.send(Request::Char('\x1B'))?,
                            VK_PRIOR => {
                                req_tx.send(Request::Key(KeyCode::PageUp))?
                            }
                            VK_NEXT => {
                                req_tx.send(Request::Key(KeyCode::PageDown))?
                            }
                            VK_HOME => {
                                req_tx.send(Request::Key(KeyCode::Home))?
                            }
                            VK_END => {
                                req_tx.send(Request::Key(KeyCode::End))?
                            }
                            VK_LEFT => {
                                req_tx.send(Request::Key(KeyCode::Left))?
                            }
                            VK_RIGHT => {
                                req_tx.send(Request::Key(KeyCode::Right))?
                            }
                            VK_UP => req_tx.send(Request::Key(KeyCode::Up))?,
                            VK_DOWN => {
                                req_tx.send(Request::Key(KeyCode::Down))?
                            }
                            // TODO: VK_INSERT
                            VK_DELETE => {
                                req_tx.send(Request::Key(KeyCode::Delete))?
                            }
                            _ => match as_code_unit(event.uChar.UnicodeChar) {
                                UTF16CodeUnit::Char(0) => (),
                                UTF16CodeUnit::Char(ch) => {
                                    req_tx.send(Request::Char(
                                        (ch as u32).try_into().unwrap(),
                                    ))?;
                                }
                                UTF16CodeUnit::High(x) => {
                                    if high_surrogate.take().is_some() {
                                        req_tx
                                            .send(Request::Char('\u{fffd}'))?;
                                    }
                                    high_surrogate = Some(x);
                                }
                                UTF16CodeUnit::Low(x) => {
                                    if let Some(high_surrogate) =
                                        high_surrogate.take()
                                    {
                                        let code_point = 0x10000
                                            | ((high_surrogate as u32) << 10)
                                            | (x as u32);
                                        req_tx.send(Request::Char(
                                            code_point.try_into().unwrap(),
                                        ))?;
                                    } else {
                                        req_tx
                                            .send(Request::Char('\u{fffd}'))?;
                                    }
                                }
                            },
                        }
                    }
                    WINDOW_BUFFER_SIZE_EVENT => {
                        req_tx.send(Request::Char('\x0C'))?; // ^L
                    }
                    _ => continue,
                }
            }
        }
    }
}

impl WinTerminal {
    pub(crate) fn new(
        req_tx: std_mpsc::Sender<Request>,
    ) -> Result<WinTerminal, DummyError> {
        let stdin_handle;
        let stdout_handle;
        unsafe {
            stdin_handle = GetStdHandle(STD_INPUT_HANDLE)
                .expect("unable to retrieve console input handle");
            stdout_handle = GetStdHandle(STD_OUTPUT_HANDLE)
                .expect("unable to retrieve console output handle");
            if true {
                let mut csbi = zeroed();
                GetConsoleScreenBufferInfo(stdout_handle, &raw mut csbi)?;
            }
        }
        let _input_thread = std::thread::Builder::new()
            .name("Liso input processing thread".to_owned())
            .spawn(move || {
                let _ = input_thread(
                    unsafe { GetStdHandle(STD_INPUT_HANDLE) }.unwrap(),
                    req_tx,
                );
            })
            .unwrap();
        let cur_attributes =
            FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_BLUE;
        unsafe {
            let _ = SetConsoleTextAttribute(stdout_handle, cur_attributes);
        }
        let mut ret = WinTerminal {
            stdin_handle,
            stdout_handle,
            old_hook: None,
            suspended: true,
            cur_attributes,
            cur_style: Style::PLAIN,
        };
        ret.unsuspend()?;
        Ok(ret)
    }
    fn move_cursor(&mut self, x: i16, y: i16) -> LifeOrDeath {
        unsafe {
            let mut csbi = zeroed();
            GetConsoleScreenBufferInfo(self.stdout_handle, &raw mut csbi)?;
            let new_x = x
                .saturating_add(csbi.dwCursorPosition.X)
                .max(0)
                .min(csbi.dwSize.X - 1);
            csbi.dwCursorPosition.X = new_x;
            let new_y = y.saturating_add(csbi.dwCursorPosition.Y).max(0);
            csbi.dwCursorPosition.Y = new_y.min(csbi.dwSize.Y - 1);
            SetConsoleCursorPosition(
                self.stdout_handle,
                csbi.dwCursorPosition,
            )?;
            if new_y >= csbi.dwSize.Y {
                // using ScrollConsoleScreenBuffer can't go past the bottom of
                // the screen in fake consoles, so for compatibility we use
                // WriteConsole here
                // (this shouldn't happen, as we should be using ANSI sequences
                // when we're not in a true console window)
                let scroll_amount = new_y - csbi.dwSize.Y + 1;
                WriteConsoleW(
                    self.stdout_handle,
                    &vec![0x0Au16; scroll_amount as usize],
                    None,
                    None,
                )?;
            }
        }
        Ok(())
    }
    fn interrupt_input_thread(&mut self) -> LifeOrDeath {
        unsafe {
            let event = [INPUT_RECORD {
                EventType: USER_INTERRUPT_EVENT as u16,
                Event: zeroed(),
            }];
            let mut wrote = 0;
            WriteConsoleInputW(self.stdin_handle, &event, &raw mut wrote)?;
        }
        Ok(())
    }
}

impl Term for WinTerminal {
    fn set_attrs(
        &mut self,
        style: Style,
        fg: Option<Color>,
        bg: Option<Color>,
    ) -> LifeOrDeath {
        let windows_info = &*WINDOWS_INFO;
        let (fg, bg) = if style.contains(Style::INVERSE) {
            (bg, fg)
        } else {
            (fg, bg)
        };
        let mut attributes = CONSOLE_CHARACTER_ATTRIBUTES(0);
        match fg {
            None => attributes |= windows_info.default_fg,
            Some(Color::Black) => (),
            Some(Color::Red) => attributes |= FOREGROUND_RED,
            Some(Color::Green) => attributes |= FOREGROUND_GREEN,
            Some(Color::Blue) => attributes |= FOREGROUND_BLUE,
            Some(Color::Cyan) => {
                attributes |= FOREGROUND_GREEN | FOREGROUND_BLUE
            }
            Some(Color::Magenta) => {
                attributes |= FOREGROUND_RED | FOREGROUND_BLUE
            }
            Some(Color::Yellow) => {
                attributes |= FOREGROUND_GREEN | FOREGROUND_BLUE
            }
            Some(Color::White) => {
                attributes |=
                    FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_BLUE
            }
        };
        match bg {
            None => attributes |= windows_info.default_bg,
            Some(Color::Black) => (),
            Some(Color::Red) => attributes |= BACKGROUND_RED,
            Some(Color::Green) => attributes |= BACKGROUND_GREEN,
            Some(Color::Blue) => attributes |= BACKGROUND_BLUE,
            Some(Color::Cyan) => {
                attributes |= BACKGROUND_GREEN | BACKGROUND_BLUE
            }
            Some(Color::Magenta) => {
                attributes |= BACKGROUND_RED | BACKGROUND_BLUE
            }
            Some(Color::Yellow) => {
                attributes |= BACKGROUND_GREEN | BACKGROUND_BLUE
            }
            Some(Color::White) => {
                attributes |=
                    BACKGROUND_RED | BACKGROUND_GREEN | BACKGROUND_BLUE
            }
        };
        if style.contains(Style::BOLD) {
            attributes |= FOREGROUND_INTENSITY;
        }
        if style.contains(Style::UNDERLINE) {
            attributes |= COMMON_LVB_UNDERSCORE;
        }
        if self.cur_attributes != attributes {
            unsafe {
                let _ =
                    SetConsoleTextAttribute(self.stdout_handle, attributes);
            }
            self.cur_attributes = attributes;
        }
        self.cur_style = style
            .intersection(Style::BOLD | Style::UNDERLINE | Style::INVERSE);
        Ok(())
    }
    fn reset_attrs(&mut self) -> LifeOrDeath {
        self.set_attrs(Style::PLAIN, None, None)
    }
    fn print(&mut self, text: &str) -> LifeOrDeath {
        let as_u16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            WriteConsoleW(self.stdout_handle, &as_u16, None, None)?;
        }
        Ok(())
    }
    fn print_char(&mut self, ch: char) -> LifeOrDeath {
        let mut buf = [0u16; 2];
        let slice = ch.encode_utf16(&mut buf);
        unsafe {
            WriteConsoleW(self.stdout_handle, slice, None, None)?;
        }
        Ok(())
    }
    fn print_spaces(&mut self, spaces: usize) -> LifeOrDeath {
        // TODO: Use FillConsoleOutputCharacterW instead?
        let buf = vec![0x20u16; spaces];
        unsafe {
            WriteConsoleW(self.stdout_handle, &buf, None, None)?;
        }
        Ok(())
    }
    fn move_cursor_up(&mut self, amt: u32) -> LifeOrDeath {
        self.move_cursor(0, -(amt.min(32767) as i16))
    }
    fn move_cursor_down(&mut self, amt: u32) -> LifeOrDeath {
        self.move_cursor(0, amt.min(32767) as i16)
    }
    fn move_cursor_left(&mut self, amt: u32) -> LifeOrDeath {
        self.move_cursor(-(amt.min(32767) as i16), 0)
    }
    fn move_cursor_right(&mut self, amt: u32) -> LifeOrDeath {
        self.move_cursor(amt.min(32767) as i16, 0)
    }
    fn cur_style(&self) -> Style {
        self.cur_style
    }
    fn newline(&mut self) -> LifeOrDeath {
        self.move_cursor(-32768, 1)
    }
    fn carriage_return(&mut self) -> LifeOrDeath {
        self.move_cursor(-32768, 0)
    }
    fn bell(&mut self) -> LifeOrDeath {
        unsafe {
            // use WriteConsoleW for compatibility with hacky remote consoles
            WriteConsoleW(self.stdout_handle, &[0x07u16], None, None)?;
        }
        Ok(())
    }
    fn clear_all_and_reset(&mut self) -> LifeOrDeath {
        self.reset_attrs()?;
        unsafe {
            let mut csbi = zeroed();
            GetConsoleScreenBufferInfo(self.stdout_handle, &raw mut csbi)?;
            let scroll_rect = SMALL_RECT {
                Left: 0,
                Top: 0,
                Right: csbi.dwSize.X,
                Bottom: csbi.dwSize.Y,
            };
            let scroll_amount = COORD {
                X: 0,
                Y: -scroll_rect.Bottom,
            };
            let fill = CHAR_INFO {
                Char: CHAR_INFO_0 { UnicodeChar: 0x20 },
                Attributes: 0,
            };
            ScrollConsoleScreenBufferW(
                self.stdout_handle,
                &scroll_rect,
                None,
                scroll_amount,
                &fill,
            )?;
            SetConsoleCursorPosition(
                self.stdout_handle,
                COORD { X: 0, Y: 0 },
            )?;
        }
        Ok(())
    }
    fn clear_forward_and_reset(&mut self) -> LifeOrDeath {
        self.reset_attrs()?;
        unsafe {
            let mut csbi = zeroed();
            GetConsoleScreenBufferInfo(self.stdout_handle, &raw mut csbi)?;
            let (w, h) = (csbi.dwSize.X as i32, csbi.dwSize.Y as i32);
            let (x, y) = (
                csbi.dwCursorPosition.X as i32,
                csbi.dwCursorPosition.Y as i32,
            );
            let mut chars_written = 0;
            let amt = ((h - y) * w + (w - x)) as u32;
            FillConsoleOutputCharacterW(
                self.stdout_handle,
                0x20u16,
                amt,
                csbi.dwCursorPosition,
                &raw mut chars_written,
            )?;
            FillConsoleOutputAttribute(
                self.stdout_handle,
                0,
                amt,
                csbi.dwCursorPosition,
                &raw mut chars_written,
            )?;
        }
        Ok(())
    }
    fn clear_to_end_of_line(&mut self) -> LifeOrDeath {
        unsafe {
            let mut csbi = zeroed();
            GetConsoleScreenBufferInfo(self.stdout_handle, &raw mut csbi)?;
            let (w, _h) = (csbi.dwSize.X as i32, csbi.dwSize.Y as i32);
            let (x, _y) = (
                csbi.dwCursorPosition.X as i32,
                csbi.dwCursorPosition.Y as i32,
            );
            let amt = (w - x) as u32;
            let mut chars_written = 0;
            FillConsoleOutputCharacterW(
                self.stdout_handle,
                0x20u16,
                amt,
                csbi.dwCursorPosition,
                &raw mut chars_written,
            )?;
            FillConsoleOutputAttribute(
                self.stdout_handle,
                0,
                amt,
                csbi.dwCursorPosition,
                &raw mut chars_written,
            )?;
        }
        Ok(())
    }
    fn hide_cursor(&mut self) -> LifeOrDeath {
        unsafe {
            let mut cci = zeroed();
            GetConsoleCursorInfo(self.stdout_handle, &raw mut cci)?;
            cci.bVisible = BOOL(0);
            SetConsoleCursorInfo(self.stdout_handle, &raw mut cci)?;
        }
        Ok(())
    }
    fn show_cursor(&mut self) -> LifeOrDeath {
        unsafe {
            let mut cci = zeroed();
            GetConsoleCursorInfo(self.stdout_handle, &raw mut cci)?;
            cci.bVisible = BOOL(1);
            SetConsoleCursorInfo(self.stdout_handle, &raw mut cci)?;
        }
        Ok(())
    }
    fn get_width(&mut self) -> u32 {
        unsafe {
            let mut csbi = zeroed();
            let Ok(_) =
                GetConsoleScreenBufferInfo(self.stdout_handle, &raw mut csbi)
            else {
                return 80;
            };
            csbi.dwSize.X as u32
        }
    }
    fn flush(&mut self) -> LifeOrDeath {
        Ok(())
    }
    fn unsuspend(&mut self) -> LifeOrDeath {
        assert!(self.suspended);
        let old_hook = panic::take_hook();
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            crate::exit_raw_mode();
            default_hook(info)
        }));
        crate::enter_raw_mode(false);
        let _ = self.hide_cursor();
        self.suspended = false;
        self.old_hook = Some(old_hook);
        Ok(())
    }
    fn suspend(&mut self) -> LifeOrDeath {
        assert!(!self.suspended);
        let _ = self.show_cursor();
        let _ = self.clear_forward_and_reset();
        crate::exit_raw_mode();
        if let Some(old_hook) = self.old_hook.take() {
            panic::set_hook(old_hook);
        }
        self.suspended = true;
        Ok(())
    }
    fn cleanup(&mut self) -> LifeOrDeath {
        if !self.suspended {
            self.suspend()?;
        }
        self.interrupt_input_thread()?;
        Ok(())
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum UTF16CodeUnit {
    Low(u16),
    High(u16),
    Char(u16),
}

fn as_code_unit(x: u16) -> UTF16CodeUnit {
    if (0xD800..=0xDFFF).contains(&x) {
        if (0xD800..=0xDBFF).contains(&x) {
            UTF16CodeUnit::High(x - 0xD800)
        } else {
            UTF16CodeUnit::Low(x - 0xDC00)
        }
    } else {
        UTF16CodeUnit::Char(x)
    }
}
