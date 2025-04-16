use super::*;

use std::{
    io::{ErrorKind, Write},
    panic,
};

use crossterm::{event::KeyEvent, *};
use std::result::Result; // override crossterm::Result
use unicode_width::UnicodeWidthChar;

/// Talks to a VT52, or (way more likely) an Atari ST (or descendant) emulating
/// one.
pub(crate) struct Vt52 {
    suspended: bool,
    old_hook:
        Option<Box<dyn Fn(&panic::PanicHookInfo<'_>) + Sync + Send + 'static>>,
    stdout: Stdout,
    num_colors: u8,
    cur_style: Style,
    cur_fg: u8,
    cur_bg: u8,
    white_on_black: bool,
    input_thread: InterruptibleStdinThread,
}

fn input_thread(
    input_rx: std_mpsc::Receiver<Vec<u8>>,
    req_tx: std_mpsc::Sender<Request>,
) -> LifeOrDeath {
    let mut buf = Vec::new();
    loop {
        let wort = input_rx.recv()?;
        if buf.is_empty() {
            buf = wort;
        } else {
            buf.extend_from_slice(&wort[..]);
        }
        let mut start = 0;
        while start < buf.len() {
            const ESCAPE: u8 = 0x1B;
            if buf[start] == ESCAPE {
                // Begin escape sequence processing
                if start + 1 >= buf.len() {
                    // Read more data, on deadline
                    match input_rx.recv_timeout(ESCAPE_DELAY) {
                        Ok(x) => buf.extend_from_slice(&x[..]),
                        Err(std_mpsc::RecvTimeoutError::Timeout) => (),
                        _ => return Ok(()),
                    }
                }
                if start + 1 >= buf.len()
                    || buf[start + 1] < 0x20
                    || buf[start + 1] >= 0x7F
                {
                    // Just send the escape
                    start += 1;
                    let event = KeyEvent {
                        code: event::KeyCode::Esc,
                        modifiers: event::KeyModifiers::empty(),
                        kind: event::KeyEventKind::Press,
                        state: event::KeyEventState::empty(),
                    };
                    let event = Event::Key(event);
                    req_tx.send(Request::CrosstermEvent(event))?;
                    continue;
                }
                // all VT52 escape sequences are two-byte.
                match buf[start + 1] {
                    b'/' => {
                        // ...except this one!
                        // This is sent in response to the "identify yourself"
                        // request.
                        if start + 2 >= buf.len() {
                            // more input needed!
                            break;
                        }
                        start += 3;
                        continue;
                    }
                    // TODO: sequences
                    _ => (),
                }
                start += 2;
            } else if buf[start] >= 0x80 {
                // We have to discard the meta bytes we receive
                // TODO: are these sent sometimes?
            } else {
                let mut text_end = start + 1;
                while text_end < buf.len()
                    && buf[text_end] < 0x80
                    && buf[text_end] != ESCAPE
                {
                    text_end += 1;
                }
                let text =
                    String::from_utf8_lossy(&buf[start..text_end]).to_string();
                req_tx.send(Request::RawInput(text))?;
                start = text_end;
            }
        }
        if start < buf.len() {
            let buf_len = buf.len();
            buf.copy_within(start..buf_len, 0);
            buf.truncate(buf.len() - start);
        } else {
            buf.clear();
        }
    }
}

impl Vt52 {
    pub(crate) fn new(
        req_tx: std_mpsc::Sender<Request>,
        num_colors: u8,
    ) -> Result<Vt52, DummyError> {
        let white_on_black = match std::env::var("ATARI_WHITE_ON_BLACK")
            .as_ref()
            .map(String::as_str)
        {
            Err(_) => false,
            Ok("0") => false,
            Ok("1") => true,
            Ok(x) if x.starts_with('n') || x.starts_with('N') => false,
            Ok(x) if x.starts_with('f') || x.starts_with('F') => false,
            Ok(x) if x.starts_with('y') || x.starts_with('Y') => true,
            Ok(x) if x.starts_with('t') || x.starts_with('T') => true,
            Ok(_) => {
                eprintln!(
                    "Unrecognized value for ATARI_WHITE_ON_BLACK \
                               environment variable. Using black on white."
                );
                false
            }
        };
        let (input_tx, input_rx) = std_mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("Liso raw stdin thread".to_owned())
            .spawn(move || {
                let stdin = std::io::stdin();
                let mut stdin = stdin.lock();
                let mut buf = [0u8; 256];
                loop {
                    let amt = match stdin.read(&mut buf[..]) {
                        Err(x) if x.kind() == ErrorKind::Interrupted => {
                            continue
                        } // as though nothing happened
                        Ok(0) | Err(_) => break,
                        Ok(x) => x,
                    };
                    if input_tx.send(buf[..amt].to_owned()).is_err() {
                        break;
                    }
                }
            })
            .unwrap();
        let input_thread = std::thread::Builder::new()
            .name("Liso input processing thread".to_owned())
            .spawn(move || {
                let _ = input_thread(input_rx, req_tx);
            })
            .unwrap();
        let stdout = std::io::stdout();
        let mut ret = Vt52 {
            stdout,
            old_hook: None,
            suspended: true,
            cur_style: Style::PLAIN,
            cur_fg: num_colors - 1,
            cur_bg: 0,
            num_colors,
            white_on_black,
            input_thread: InterruptibleStdinThread::new(input_thread),
        };
        ret.unsuspend()?;
        Ok(ret)
    }
}

impl Term for Vt52 {
    fn set_attrs(
        &mut self,
        style: Style,
        fg: Option<Color>,
        bg: Option<Color>,
    ) -> LifeOrDeath {
        // the only styling supported by the Atari VT52 emulator was inverse
        // video, and we end up emulating it anyway to get our bright/dim split
        // working
        let (fg, bg) = if self.white_on_black {
            match self.num_colors {
                16 => (
                    fg.map(Color::to_atari16_bright).unwrap_or(0),
                    bg.map(Color::to_atari16_dim).unwrap_or(15),
                ),
                4 => (
                    fg.map(Color::to_atari4).unwrap_or(0),
                    bg.map(Color::to_atari4).unwrap_or(15),
                ),
                2 => (0, 15),
                _ => unreachable!(),
            }
        } else {
            match self.num_colors {
                16 => (
                    fg.map(Color::to_atari16_dim).unwrap_or(15),
                    bg.map(Color::to_atari16_bright).unwrap_or(0),
                ),
                4 => (
                    fg.map(Color::to_atari4).unwrap_or(15),
                    bg.map(Color::to_atari4).unwrap_or(0),
                ),
                2 => (15, 0),
                _ => unreachable!(),
            }
        };
        let (fg, bg) = if style.contains(Style::INVERSE) {
            (bg, fg)
        } else {
            (fg, bg)
        };
        self.cur_style = style;
        if fg != self.cur_fg {
            self.cur_fg = fg;
            write!(self.stdout, "\x1Bb{}", (fg + 0x20) as char)?;
        }
        if bg != self.cur_bg {
            self.cur_bg = bg;
            write!(self.stdout, "\x1Bc{}", (bg + 0x20) as char)?;
        }
        Ok(())
    }
    fn reset_attrs(&mut self) -> LifeOrDeath {
        if self.white_on_black {
            write!(self.stdout, "\x1Bb\x20\x1Bc\x2F")?;
            self.cur_fg = 0;
            self.cur_bg = 15;
        } else {
            write!(self.stdout, "\x1Bb\x2F\x1Bc\x20")?;
            self.cur_fg = 15;
            self.cur_bg = 0;
        }
        self.cur_style = Style::PLAIN;
        Ok(())
    }
    fn print(&mut self, text: &str) -> LifeOrDeath {
        // Atari ST doesn't support Unicode. In fact, its VT52 emulator doesn't
        // even support 8 bits. So we output a weird delta character any time
        // we run into Unicode characters.
        let bytes = text.as_bytes();
        let mut start = 0;
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] >= 0x80 {
                if pos != start {
                    self.stdout.write_all(&bytes[start..pos])?;
                }
                if bytes[pos] >= 0xC0 {
                    let ch = text[pos..].chars().nth(0).unwrap();
                    let width = UnicodeWidthChar::width(ch).unwrap_or(0);
                    for _ in 0..width {
                        self.stdout.write_all(b"\x7F")?;
                    }
                }
                pos += 1;
                start = pos;
            } else {
                pos += 1;
            }
        }
        if pos != start {
            self.stdout.write_all(&bytes[start..pos])?;
        }
        Ok(())
    }
    fn print_char(&mut self, ch: char) -> LifeOrDeath {
        if ch >= '\u{0080}' {
            self.stdout.write_all(b"\x7F")?;
        } else {
            self.stdout.write_all(&[ch as u8])?;
        }
        Ok(())
    }
    fn print_spaces(&mut self, spaces: usize) -> LifeOrDeath {
        for _ in 0..spaces {
            write!(self.stdout, " ")?;
        }
        Ok(())
    }
    fn move_cursor_up(&mut self, amt: u32) -> LifeOrDeath {
        for _ in 0..amt {
            write!(self.stdout, "\x1BA")?;
        }
        Ok(())
    }
    fn move_cursor_down(&mut self, amt: u32) -> LifeOrDeath {
        for _ in 0..amt {
            write!(self.stdout, "\x1BB")?;
        }
        Ok(())
    }
    fn move_cursor_left(&mut self, amt: u32) -> LifeOrDeath {
        for _ in 0..amt {
            write!(self.stdout, "\x1BD")?;
        }
        Ok(())
    }
    fn move_cursor_right(&mut self, amt: u32) -> LifeOrDeath {
        for _ in 0..amt {
            write!(self.stdout, "\x1BC")?;
        }
        Ok(())
    }
    fn cur_style(&self) -> Style {
        self.cur_style
    }
    fn newline(&mut self) -> LifeOrDeath {
        write!(self.stdout, "\r\n")?;
        Ok(())
    }
    fn carriage_return(&mut self) -> LifeOrDeath {
        write!(self.stdout, "\r")?;
        Ok(())
    }
    fn bell(&mut self) -> LifeOrDeath {
        write!(self.stdout, "\x07")?;
        Ok(())
    }
    fn clear_all_and_reset(&mut self) -> LifeOrDeath {
        if self.white_on_black {
            write!(self.stdout, "\x1BY  \x1Bb\x20\x1Bc\x2F\x1BJ")?;
            self.cur_fg = 0;
            self.cur_bg = 15;
        } else {
            write!(self.stdout, "\x1BY  \x1Bb\x2F\x1Bc\x20\x1BJ")?;
            self.cur_fg = 15;
            self.cur_bg = 0;
        }
        self.cur_style = Style::PLAIN;
        Ok(())
    }
    fn clear_to_end_of_line(&mut self) -> LifeOrDeath {
        write!(self.stdout, "\x1BK")?;
        Ok(())
    }
    fn clear_forward_and_reset(&mut self) -> LifeOrDeath {
        if self.white_on_black {
            write!(self.stdout, "\x1Bb\x20\x1Bc\x2F\x1BJ")?;
            self.cur_fg = 0;
            self.cur_bg = 15;
        } else {
            write!(self.stdout, "\x1Bb\x2F\x1Bc\x20\x1BJ")?;
            self.cur_fg = 15;
            self.cur_bg = 0;
        }
        self.cur_style = Style::PLAIN;
        Ok(())
    }
    fn hide_cursor(&mut self) -> LifeOrDeath {
        write!(self.stdout, "\x1Bf")?;
        Ok(())
    }
    fn show_cursor(&mut self) -> LifeOrDeath {
        write!(self.stdout, "\x1Be")?;
        Ok(())
    }
    fn get_width(&mut self) -> u32 {
        terminal::size().unwrap_or((80, 24)).0 as u32
    }
    fn flush(&mut self) -> LifeOrDeath {
        self.stdout.flush()?;
        Ok(())
    }
    fn unsuspend(&mut self) -> LifeOrDeath {
        assert!(self.suspended);
        // queue, but don't actually output anything until the first command...
        if self.white_on_black {
            self.stdout
                .write_all(b"\x1Bf\x1Bw\x1Bq\x1Bb\x20\x1Bc\x2F")?;
        } else {
            self.stdout
                .write_all(b"\x1Bf\x1Bw\x1Bq\x1Bb\x2F\x1Bc\x20")?;
        }
        let old_hook = panic::take_hook();
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let mut stdout = std::io::stdout();
            let _ = queue!(
                stdout,
                cursor::Show,
                terminal::EnableLineWrap,
                style::ResetColor,
                style::SetAttribute(CtAttribute::Reset),
                terminal::Clear(terminal::ClearType::FromCursorDown)
            );
            let _ = stdout.flush();
            let _ = terminal::disable_raw_mode();
            default_hook(info)
        }));
        terminal::enable_raw_mode()?;
        self.suspended = false;
        self.old_hook = Some(old_hook);
        Ok(())
    }
    fn suspend(&mut self) -> LifeOrDeath {
        assert!(!self.suspended);
        if self.white_on_black {
            write!(self.stdout, "\x1Bb\x20\x1Bc\x2F\x1BJ\x1Bv\x1Be")?;
        } else {
            write!(self.stdout, "\x1Bb\x2F\x1Bc\x20\x1BJ\x1Bv\x1Be")?;
        }
        self.stdout.flush()?;
        if let Some(old_hook) = self.old_hook.take() {
            panic::set_hook(old_hook);
        }
        terminal::disable_raw_mode()?;
        self.suspended = true;
        Ok(())
    }
    fn cleanup(&mut self) -> LifeOrDeath {
        if !self.suspended {
            self.suspend()?;
        }
        self.input_thread.interrupt();
        Ok(())
    }
}
