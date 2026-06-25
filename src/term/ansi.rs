use super::*;

use std::{
    io::{ErrorKind, Write},
    panic,
};

use super::KeyCode;

/// Uses ANSI escape sequences for input and output.
pub(crate) struct AnsiTerminal {
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
    req_tx.send(Request::Key(code))?;
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
                    req_tx.send(Request::Char('\x1B'))?;
                    continue;
                }
                match buf[start + 1] {
                    b'[' => {
                        // multi-char sequence
                        let mut seq_end = None;
                        for (i, &b) in buf.iter().enumerate().skip(start + 2) {
                            if !(0x20..0x40).contains(&b) {
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
                    req_tx.send(Request::Char('\u{fffd}'))?;
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
                        req_tx.send(Request::Char('\u{fffd}'))?;
                        continue 'processing;
                    }
                    code = (code << 6) | (buf[start + i] & 0x3F) as u32;
                }
                start += num_bytes_needed;
                // send the decoded character
                let code = if code > 0x10FFFF { 0xFFFD } else { code };
                let ch = char::from_u32(code).unwrap();
                req_tx.send(Request::Char(ch))?;
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

impl AnsiTerminal {
    pub(crate) fn new(
        req_tx: std_mpsc::Sender<Request>,
    ) -> Result<AnsiTerminal, DummyError> {
        let (input_tx, input_rx) = std_mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("Liso input processing thread".to_owned())
            .spawn(move || {
                let _ = input_thread(input_rx, req_tx);
            })
            .unwrap();
        let input_thread = std::thread::Builder::new()
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
        let stdout = std::io::stdout();
        let mut ret = AnsiTerminal {
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

impl Term for AnsiTerminal {
    fn set_attrs(
        &mut self,
        style: Style,
        fg: Option<Color>,
        bg: Option<Color>,
    ) -> LifeOrDeath {
        let style = if style.contains(Style::BOLD | Style::DIM) {
            style - Style::DIM
        } else {
            style
        };
        let mut piecemeal_gubbins = Vec::with_capacity(7);
        let styles_to_set = style & !self.cur_style;
        let styles_to_clear = self.cur_style & !style;
        if styles_to_set.contains(Style::BOLD) {
            piecemeal_gubbins.push("1");
        } else if styles_to_clear.contains(Style::BOLD) {
            piecemeal_gubbins.push("22"); // NOT 21
        }
        if styles_to_set.contains(Style::DIM) {
            piecemeal_gubbins.push("2");
        } else if styles_to_clear.contains(Style::DIM)
            && !style.contains(Style::BOLD)
        {
            piecemeal_gubbins.push("22");
        }
        if styles_to_set.contains(Style::UNDERLINE) {
            piecemeal_gubbins.push("4");
        } else if styles_to_clear.contains(Style::UNDERLINE) {
            piecemeal_gubbins.push("24");
        }
        if styles_to_set.contains(Style::INVERSE) {
            piecemeal_gubbins.push("7");
        } else if styles_to_clear.contains(Style::INVERSE) {
            piecemeal_gubbins.push("27");
        }
        if styles_to_set.contains(Style::ITALIC) {
            piecemeal_gubbins.push("3");
        } else if styles_to_clear.contains(Style::ITALIC) {
            piecemeal_gubbins.push("23");
        }
        if fg != self.cur_fg {
            piecemeal_gubbins.push(fg.map(|x| x.as_ansi_fg()).unwrap_or("39"));
        }
        if bg != self.cur_bg {
            piecemeal_gubbins.push(fg.map(|x| x.as_ansi_bg()).unwrap_or("49"));
        }
        let piecemeal = gubbins_to_sequence(&piecemeal_gubbins);
        let mut flockmeal_gubbins = Vec::with_capacity(8);
        flockmeal_gubbins.push("0");
        if style.contains(Style::BOLD) {
            flockmeal_gubbins.push("1");
        }
        if style.contains(Style::DIM) {
            flockmeal_gubbins.push("2");
        }
        if style.contains(Style::UNDERLINE) {
            flockmeal_gubbins.push("4");
        }
        if style.contains(Style::INVERSE) {
            flockmeal_gubbins.push("7");
        }
        if style.contains(Style::ITALIC) {
            flockmeal_gubbins.push("3");
        }
        if let Some(fg) = fg {
            flockmeal_gubbins.push(fg.as_ansi_fg());
        }
        if let Some(bg) = bg {
            flockmeal_gubbins.push(bg.as_ansi_bg());
        }
        let flockmeal = gubbins_to_sequence(&flockmeal_gubbins);
        if flockmeal.len() <= piecemeal.len() {
            // flockmeal should win all else being equal, as it is slightly
            // less likely to go wrong
            self.stdout.write_all(flockmeal.as_bytes())?;
        } else {
            self.stdout.write_all(piecemeal.as_bytes())?;
        }
        self.cur_style = style;
        self.cur_fg = fg;
        self.cur_bg = bg;
        Ok(())
    }
    fn reset_attrs(&mut self) -> LifeOrDeath {
        self.set_attrs(Style::PLAIN, None, None)
    }
    fn print(&mut self, text: &str) -> LifeOrDeath {
        self.stdout.write_all(text.as_bytes())?;
        Ok(())
    }
    fn print_char(&mut self, ch: char) -> LifeOrDeath {
        write!(self.stdout, "{ch}")?;
        Ok(())
    }
    fn print_spaces(&mut self, spaces: usize) -> LifeOrDeath {
        // TODO: ...kinda inefficient eh?
        for _ in 0..spaces {
            write!(self.stdout, " ")?;
        }
        Ok(())
    }
    fn move_cursor_up(&mut self, amt: u32) -> LifeOrDeath {
        write!(self.stdout, "\x1B[{amt}A")?;
        Ok(())
    }
    fn move_cursor_down(&mut self, amt: u32) -> LifeOrDeath {
        write!(self.stdout, "\x1B[{amt}B")?;
        Ok(())
    }
    fn move_cursor_left(&mut self, amt: u32) -> LifeOrDeath {
        write!(self.stdout, "\x1B[{amt}D")?;
        Ok(())
    }
    fn move_cursor_right(&mut self, amt: u32) -> LifeOrDeath {
        write!(self.stdout, "\x1B[{amt}C")?;
        Ok(())
    }
    fn cur_style(&self) -> Style {
        self.cur_style
    }
    fn newline(&mut self) -> LifeOrDeath {
        self.stdout.write_all(b"\r\n")?;
        Ok(())
    }
    fn carriage_return(&mut self) -> LifeOrDeath {
        self.stdout.write_all(b"\r")?;
        Ok(())
    }
    fn bell(&mut self) -> LifeOrDeath {
        self.stdout.write_all(b"\x07")?;
        Ok(())
    }
    fn clear_all_and_reset(&mut self) -> LifeOrDeath {
        self.reset_attrs()?;
        self.stdout.write_all(b"\x1B[2J\x1B[H")?;
        Ok(())
    }
    fn clear_forward_and_reset(&mut self) -> LifeOrDeath {
        self.reset_attrs()?;
        self.stdout.write_all(b"\x1B[J")?;
        Ok(())
    }
    fn clear_to_end_of_line(&mut self) -> LifeOrDeath {
        self.stdout.write_all(b"\x1B[K")?;
        Ok(())
    }
    fn hide_cursor(&mut self) -> LifeOrDeath {
        self.stdout.write_all(b"\x1B[25h")?;
        Ok(())
    }
    fn show_cursor(&mut self) -> LifeOrDeath {
        self.stdout.write_all(b"\x1B[25l")?;
        Ok(())
    }
    fn get_width(&mut self) -> u32 {
        termsize::get().map(|x| x.cols as u32).unwrap_or(80)
    }
    fn flush(&mut self) -> LifeOrDeath {
        self.stdout.flush()?;
        Ok(())
    }
    fn unsuspend(&mut self) -> LifeOrDeath {
        assert!(self.suspended);
        // hide cursor, disable line wrap, reset style
        self.stdout.write_all(b"\x1B[25h\x1B[7l\x1B[0m")?;
        let old_hook = panic::take_hook();
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let mut stdout = std::io::stdout();
            // show cursor, enable line wrap, reset style, clear forward
            let _ = stdout.write_all(b"\x1B[25l\x1B[7h\x1B[0m\x1B[J");
            let _ = stdout.flush();
            crate::exit_raw_mode();
            default_hook(info)
        }));
        // don't bother checking failure?
        crate::enter_raw_mode();
        self.suspended = false;
        self.old_hook = Some(old_hook);
        Ok(())
    }
    fn suspend(&mut self) -> LifeOrDeath {
        assert!(!self.suspended);
        // show cursor, enable line wrap, reset style, clear forward
        self.stdout.write_all(b"\x1B[25l\x1B[7h\x1B[0m\x1B[J")?;
        crate::exit_raw_mode();
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

fn gubbins_to_sequence(gubbins: &[&str]) -> String {
    if gubbins.is_empty() {
        return String::new();
    }
    let mut ret = String::with_capacity(
        gubbins.iter().map(|x| x.len() + 1).sum::<usize>() + 2,
    );
    ret.push('\x1B');
    for (i, gubbin) in gubbins.iter().enumerate() {
        if i == 0 {
            ret.push('[');
        } else {
            ret.push(';');
        };
        ret += gubbin;
    }
    ret.push('m');
    ret
}
