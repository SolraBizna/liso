use super::*;

use std::{
    io::{ErrorKind, Write},
    panic,
};

use crossterm::{event::KeyEvent, style::Colors, *};
use std::result::Result; // override crossterm::Result

/// Uses `crossterm` for input and output.
pub(crate) struct Crossterminal {
    suspended: bool,
    old_hook:
        Option<Box<dyn Fn(&panic::PanicHookInfo<'_>) + Sync + Send + 'static>>,
    stdout: Stdout,
    cur_style: Style,
    cur_fg: Option<Color>,
    cur_bg: Option<Color>,
    input_thread: InterruptibleStdinThread,
}

fn parse_csi_sequence(
    seq: &[u8],
    req_tx: &mut std_mpsc::Sender<Request>,
) -> LifeOrDeath {
    use event::KeyCode;
    let code = match seq {
        b"[A" => KeyCode::Up,
        b"[B" => KeyCode::Down,
        b"[C" => KeyCode::Right,
        b"[D" => KeyCode::Left,
        b"[3~" => KeyCode::Delete,
        b"[H" => KeyCode::Home,
        b"[F" => KeyCode::End,
        _ => return Ok(()), // unknown
    };
    let event = KeyEvent {
        code,
        modifiers: event::KeyModifiers::empty(),
        kind: event::KeyEventKind::Press,
        state: event::KeyEventState::empty(),
    };
    let event = Event::Key(event);
    req_tx.send(Request::CrosstermEvent(event))?;
    Ok(())
}

fn input_thread(
    input_rx: std_mpsc::Receiver<Vec<u8>>,
    mut req_tx: std_mpsc::Sender<Request>,
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
        'processing: while start < buf.len() {
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
                match buf[start + 1] {
                    b'[' => {
                        // multi-char sequence
                        let mut seq_end = None;
                        for i in start + 2..buf.len() {
                            if buf[i] < 0x20 || buf[i] >= 0x40 {
                                seq_end = Some(i + 1);
                                break;
                            }
                        }
                        match seq_end {
                            Some(end) => {
                                parse_csi_sequence(
                                    &buf[start + 1..end],
                                    &mut req_tx,
                                )?;
                                start = end;
                            }
                            // we open ourselves to a memory exhaustion attack
                            // if a malicious, never-ending CSI sequence is
                            // sent. But whatever. We already have one of those
                            // for our *actual input*, unavoidably. Note to
                            // someone hoping to patch this to avoid memory
                            // exhaustion attacks in the future: add something
                            // for that here, too.
                            None => break, // more input needed
                        }
                    }
                    _ => {
                        // single-char sequence
                        // (which we don't handle)
                        start += 2;
                    }
                }
            } else if buf[start] >= 0x80 {
                // UTF-8 sequence processing
                let b = buf[start];
                let num_bytes_needed = if b >= 0xF0 {
                    4
                } else if b >= 0xE0 {
                    3
                } else if b >= 0xC0 {
                    2
                } else {
                    // send the replacement character
                    let event = KeyEvent {
                        code: event::KeyCode::Char('\u{fffd}'),
                        modifiers: event::KeyModifiers::empty(),
                        kind: event::KeyEventKind::Press,
                        state: event::KeyEventState::empty(),
                    };
                    let event = Event::Key(event);
                    req_tx.send(Request::CrosstermEvent(event))?;
                    start += 1;
                    continue;
                };
                if (buf.len() - start) < num_bytes_needed {
                    // Read more data before sending this along
                    break;
                }
                let mut code = (b & (0b1111111 >> num_bytes_needed)) as u32;
                for i in 1..num_bytes_needed {
                    if buf[start + i] < 0x80 || buf[start + i] >= 0xC0 {
                        start += i;
                        // send the replacement character
                        let event = KeyEvent {
                            code: event::KeyCode::Char('\u{fffd}'),
                            modifiers: event::KeyModifiers::empty(),
                            kind: event::KeyEventKind::Press,
                            state: event::KeyEventState::empty(),
                        };
                        let event = Event::Key(event);
                        req_tx.send(Request::CrosstermEvent(event))?;
                        continue 'processing;
                    }
                    code = (code << 6) | (buf[start + i] & 0x3F) as u32;
                }
                start += num_bytes_needed;
                // send the decoded character
                let code = if code > 0x10FFFF { 0xFFFD } else { code };
                let ch = char::from_u32(code).unwrap();
                let event = KeyEvent {
                    code: event::KeyCode::Char(ch),
                    modifiers: event::KeyModifiers::empty(),
                    kind: event::KeyEventKind::Press,
                    state: event::KeyEventState::empty(),
                };
                let event = Event::Key(event);
                req_tx.send(Request::CrosstermEvent(event))?;
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

impl Crossterminal {
    pub(crate) fn new(
        req_tx: std_mpsc::Sender<Request>,
    ) -> Result<Crossterminal, DummyError> {
        let default_crossterm_input = cfg!(windows);
        let crossterm_input = match std::env::var("LISO_CROSSTERM_INPUT")
            .as_ref()
            .map(String::as_str)
        {
            Err(_) => default_crossterm_input,
            Ok("0") => false,
            Ok("1") => true,
            Ok(x) if x.starts_with('n') || x.starts_with('N') => false,
            Ok(x) if x.starts_with('f') || x.starts_with('F') => false,
            Ok(x) if x.starts_with('y') || x.starts_with('Y') => true,
            Ok(x) if x.starts_with('t') || x.starts_with('T') => true,
            Ok(_) => {
                eprintln!(
                    "Unrecognized value for LISO_CROSSTERM_INPUT \
                               environment variable. Using the default ({}).",
                    default_crossterm_input
                );
                default_crossterm_input
            }
        };
        let input_thread = if crossterm_input {
            std::thread::Builder::new()
                .name("Liso input thread".to_owned())
                .spawn(move || {
                    while let Ok(event) = crossterm::event::read() {
                        if req_tx.send(Request::CrosstermEvent(event)).is_err()
                        {
                            break;
                        }
                    }
                })
                .unwrap()
        } else {
            let (input_tx, input_rx) = std_mpsc::sync_channel(1);
            std::thread::Builder::new()
                .name("Liso input processing thread".to_owned())
                .spawn(move || {
                    let _ = input_thread(input_rx, req_tx);
                })
                .unwrap();
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
                .unwrap()
        };
        let stdout = std::io::stdout();
        let mut ret = Crossterminal {
            stdout,
            old_hook: None,
            suspended: true,
            cur_style: Style::PLAIN,
            cur_fg: None,
            cur_bg: None,
            input_thread: InterruptibleStdinThread::new(input_thread),
        };
        ret.unsuspend()?;
        Ok(ret)
    }
}

impl Term for Crossterminal {
    fn set_attrs(
        &mut self,
        style: Style,
        fg: Option<Color>,
        bg: Option<Color>,
    ) -> LifeOrDeath {
        if self.cur_style != style
            || (self.cur_fg != fg && fg.is_none())
            || (self.cur_bg != bg && fg.is_none())
        {
            queue!(self.stdout, style::SetAttribute(CtAttribute::Reset))?;
            // TODO: check if this is needed
            if cfg!(windows) {
                queue!(
                    self.stdout,
                    style::SetColors(Colors {
                        foreground: Some(CtColor::Reset),
                        background: Some(CtColor::Reset),
                    })
                )?;
            }
            self.cur_style = Style::PLAIN;
            self.cur_fg = None;
            self.cur_bg = None;
        }
        if style != Style::PLAIN {
            let attributes = style.as_crossterm();
            queue!(self.stdout, style::SetAttributes(attributes))?;
            self.cur_style = style;
        }
        if fg != self.cur_fg || bg != self.cur_bg {
            let element_colors = Colors {
                foreground: fg.map(Color::to_crossterm),
                background: bg.map(Color::to_crossterm),
            };
            queue!(self.stdout, style::SetColors(element_colors))?;
            self.cur_fg = fg;
            self.cur_bg = bg;
        }
        Ok(())
    }
    fn reset_attrs(&mut self) -> LifeOrDeath {
        queue!(self.stdout, style::SetAttribute(CtAttribute::Reset))?;
        self.cur_style = Style::PLAIN;
        self.cur_fg = None;
        self.cur_bg = None;
        Ok(())
    }
    fn print(&mut self, text: &str) -> LifeOrDeath {
        queue!(self.stdout, style::Print(text))?;
        Ok(())
    }
    fn print_char(&mut self, ch: char) -> LifeOrDeath {
        queue!(self.stdout, style::Print(ch))?;
        Ok(())
    }
    fn print_spaces(&mut self, spaces: usize) -> LifeOrDeath {
        for _ in 0..spaces {
            queue!(self.stdout, style::Print(" "))?;
        }
        Ok(())
    }
    fn move_cursor_up(&mut self, amt: u32) -> LifeOrDeath {
        queue!(self.stdout, cursor::MoveUp(amt as u16))?;
        Ok(())
    }
    fn move_cursor_down(&mut self, amt: u32) -> LifeOrDeath {
        queue!(self.stdout, cursor::MoveDown(amt as u16))?;
        Ok(())
    }
    fn move_cursor_left(&mut self, amt: u32) -> LifeOrDeath {
        queue!(self.stdout, cursor::MoveLeft(amt as u16))?;
        Ok(())
    }
    fn move_cursor_right(&mut self, amt: u32) -> LifeOrDeath {
        queue!(self.stdout, cursor::MoveRight(amt as u16))?;
        Ok(())
    }
    fn cur_style(&self) -> Style {
        self.cur_style
    }
    fn newline(&mut self) -> LifeOrDeath {
        queue!(self.stdout, style::Print("\r\n"))?;
        Ok(())
    }
    fn carriage_return(&mut self) -> LifeOrDeath {
        queue!(self.stdout, style::Print("\r"))?;
        Ok(())
    }
    fn bell(&mut self) -> LifeOrDeath {
        queue!(self.stdout, style::Print("\u{0007}"))?;
        Ok(())
    }
    fn clear_all_and_reset(&mut self) -> LifeOrDeath {
        queue!(
            self.stdout,
            style::SetAttribute(CtAttribute::Reset),
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0)
        )?;
        self.cur_style = Style::PLAIN;
        self.cur_fg = None;
        self.cur_bg = None;
        Ok(())
    }
    fn clear_forward_and_reset(&mut self) -> LifeOrDeath {
        queue!(
            self.stdout,
            style::SetAttribute(CtAttribute::Reset),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )?;
        self.cur_style = Style::PLAIN;
        self.cur_fg = None;
        self.cur_bg = None;
        Ok(())
    }
    fn clear_to_end_of_line(&mut self) -> LifeOrDeath {
        queue!(
            self.stdout,
            terminal::Clear(terminal::ClearType::UntilNewLine)
        )?;
        Ok(())
    }
    fn hide_cursor(&mut self) -> LifeOrDeath {
        queue!(self.stdout, cursor::Hide)?;
        Ok(())
    }
    fn show_cursor(&mut self) -> LifeOrDeath {
        queue!(self.stdout, cursor::Show)?;
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
        queue!(
            self.stdout,
            cursor::Hide,
            terminal::DisableLineWrap,
            style::ResetColor,
            style::SetAttribute(CtAttribute::Reset)
        )?;
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
        queue!(
            self.stdout,
            cursor::Show,
            terminal::EnableLineWrap,
            style::ResetColor,
            style::SetAttribute(CtAttribute::Reset),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )?;
        terminal::disable_raw_mode()?;
        self.cur_style = Style::PLAIN;
        self.cur_fg = None;
        self.cur_bg = None;
        self.stdout.flush()?;
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
        self.input_thread.interrupt();
        Ok(())
    }
}
