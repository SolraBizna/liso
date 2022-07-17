//! Reads input, writes output. Communicates with your application by channels.

use super::*;

use std::{
    cell::RefCell,
    io::BufRead,
    mem::swap,
    time::Instant,
};

use unicode_width::UnicodeWidthChar;
use crossterm::tty::IsTty;

/// This is the actual worker used when we're in "pipe mode". That means we
/// either have a dumb terminal or a piped stdin/stdout.
fn pipe_worker(req_tx: std_mpsc::Sender<Request>,
               rx: std_mpsc::Receiver<Request>,
               tx: tokio_mpsc::UnboundedSender<Response>)
    -> LifeOrDeath {
    std::thread::Builder::new().name("Liso input thread".to_owned())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut stdin = stdin.lock();
            loop {
                let mut buf = String::new();
                match stdin.read_line(&mut buf) {
                    Ok(_) => {
                        while buf.ends_with('\n') || buf.ends_with('\r') {
                            buf.pop();
                        }
                        if let Err(_) = req_tx.send(Request::RawInput(buf)) {
                            break;
                        }
                    },
                    Err(_) => break,
                }
            }
        }).unwrap();
    while let Ok(request) = rx.recv() {
        match request {
            Request::Output(line) => {
                println!("{}", line.text);
            },
            Request::RawInput(x) => {
                if let Err(_) = tx.send(Response::Input(x)) {
                    break
                }
            },
            Request::Die => break,
            _ => (),
        }
    }
    Ok(())
}

#[derive(Debug,PartialEq,Eq,PartialOrd,Ord)]
enum RollState {
    NothingShown,
    StatusShown(u32),
    EverythingShown(u32, u32),
}

struct TtyState {
    status: Option<Line>,
    prompt: Option<Line>,
    notice: Option<(Line, Instant)>,
    input: String,
    input_cursor: usize,
    input_allowed: bool,
    roll_state: RollState,
    term: RefCell<Box<dyn Term>>,
}

impl TtyState {
    // does not output a line break at the end!
    fn output_line(&self,
                   line: &Line, cur_column: &mut u32, break_count: &mut u32,
                   trailing_newline: bool, endfill: bool,
                   mut cursor_report: Option<(usize, &mut u32, &mut u32)>)
    -> LifeOrDeath {
        let mut term = self.term.borrow_mut();
        // this could all do to be optimized
        let term_width = term.get_width();
        for element in line.elements.iter() {
            term.set_attrs(element.style, element.fg, element.bg)?;
            let text = &line.text[element.start .. element.end];
            let mut cur = 0;
            for (idx, ch) in text.char_indices() {
                if let Some((target, ref mut a, ref mut b)) = cursor_report {
                    if target == (idx + element.start) {
                        **a = *cur_column;
                        **b = *break_count;
                    }
                }
                let char_width
                    = UnicodeWidthChar::width(ch).unwrap_or(0) as u32;
                if (char_width > 0 && *cur_column >= term_width)
                || ch == '\n' {
                    if cur != idx {
                        term.print(&text[cur..idx])?;
                    }
                    cur = idx;
                    if ch == '\n' { cur += 1 }
                    if *cur_column < term_width
                    && (term.cur_style().contains(Style::INVERSE)
                        || element.bg.is_some()) {
                        term.print_spaces((term_width - *cur_column) as usize)?;
                        *cur_column = term_width;
                    }
                    term.newline()?;
                    *break_count += 1;
                    *cur_column = 0;
                }
                *cur_column += char_width;
            }
            if cur != text.len() {
                term.print(&text[cur..])?;
            }
        }
        if trailing_newline || endfill {
            let trailit = match line.elements.last() {
                None => false,
                Some(el) => el.style.contains(Style::INVERSE)
                    || el.bg.is_some(),
            };
            let trail_stop_col = *cur_column;
            if trailit {
                if *cur_column < term_width {
                    term.print_spaces((term_width - *cur_column) as usize)?;
                    *cur_column = term_width;
                }
            }
            if trailing_newline {
                term.newline()?;
                *break_count += 1;
                *cur_column = 0;
            }
            else if *cur_column != trail_stop_col {
                term.move_cursor_left(*cur_column - trail_stop_col)?;
                *cur_column = trail_stop_col;
            }
        }
        if let Some((target, ref mut a, ref mut b)) = cursor_report {
            if target == line.text.len() {
                **a = *cur_column;
                **b = *break_count;
            }
        }
        Ok(())
    }
    pub fn handle(&mut self, tx: &mut tokio_mpsc::UnboundedSender<Response>,
                  ded_tx: &mut std_mpsc::SyncSender<Instant>,
                  request: Request)
    -> LifeOrDeath {
        match request {
            Request::Output(line) => {
                self.rollin()?;
                let mut cur_column = 0;
                let mut break_count = 0;
                self.output_line(&line,
                                 &mut cur_column, &mut break_count,
                                 true, true, None)?;
                self.term.borrow_mut().reset_attrs()?;
            },
            Request::Status(line) => {
                if self.status != line {
                    self.rollin()?;
                    self.status = line;
                }
            },
            Request::Notice(line, duration) => {
                self.rollin()?;
                let deadline = Instant::now() + duration;
                self.notice = Some((line, deadline));
                ded_tx.send(deadline)?;
            },
            Request::Prompt{line, input_allowed, clear_input} => {
                if self.prompt != line
                || (clear_input && !self.input.is_empty()) {
                    self.rollin()?;
                    self.prompt = line;
                    self.input_allowed = input_allowed;
                    if clear_input { self.input.clear() }
                }
            },
            Request::Bell => self.term.borrow_mut().bell()?,
            Request::RawInput(input) => {
                self.handle_input(tx, &input)?
            },
            Request::CrosstermEvent(event) => {
                self.handle_event(tx, event)?
            },
            Request::Die => return Ok(()),
            Request::Heartbeat => {
                if let Some((_, deadline)) = self.notice {
                    if Instant::now() >= deadline {
                        self.rollin()?;
                        self.notice = None;
                    }
                }
            },
        }
        Ok(())
    }
    fn cursor_on<F>(&self, f: F) -> bool
    where F: FnOnce(char) -> bool {
        self.input_cursor < self.input.len()
            && f(self.input[self.input_cursor..].chars().next().unwrap())
            
    }
    /// returns true if the cursor is currently "on" an invisible character
    ///
    /// (won't return true for the first character)
    fn cursor_on_invisible(&self) -> bool {
        self.input_cursor > 0 &&
            self.cursor_on(|x| UnicodeWidthChar::width(x).unwrap_or(0) == 0)
    }
    /// returns true if the cursor is currently "on" an invisible character OR
    /// a space character
    ///
    /// (won't return true for the first character)
    fn cursor_on_invisible_or_space(&self) -> bool {
        self.input_cursor > 0 &&
            self.cursor_on(|x| x.is_whitespace()
                           || UnicodeWidthChar::width(x).unwrap_or(0) == 0)
    }
    /// returns true if the cursor is currently "on" an invisible character OR
    /// a nonspace character
    ///
    /// (won't return true for the first character)
    fn cursor_on_invisible_or_nonspace(&self) -> bool {
        self.input_cursor > 0 &&
            self.cursor_on(|x| !x.is_whitespace()
                           || UnicodeWidthChar::width(x).unwrap_or(0) == 0)
    }
    /// returns true if the cursor is currently "on" a nonspace character
    ///
    /// (MIGHT return true for the first character)
    fn cursor_on_nonspace(&self) -> bool {
        self.cursor_on(|x| !x.is_whitespace())
    }
    fn dismiss_notice(&mut self)
        -> LifeOrDeath {
        if self.notice.is_some() {
            self.rollin()?;
            self.notice = None;
        }
        Ok(())
    }
    fn handle_char_input(&mut self,
                         ch: char)
        -> LifeOrDeath {
        self.rollin()?;
        self.notice = None;
        self.input.insert(self.input_cursor, ch);
        self.input_cursor += 1;
        while !self.input.is_char_boundary(self.input_cursor) {
            self.input_cursor += 1;
        }
        Ok(())
    }
    fn handle_right_arrow(&mut self)
        -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor < self.input.len() {
            self.rollin()?;
            self.input_cursor += 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible() {
                    self.input_cursor += 1;
                }
        }
        Ok(())
    }
    fn handle_left_arrow(&mut self)
        -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor > 0 {
            self.rollin()?;
            self.input_cursor -= 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible() {
                    self.input_cursor -= 1;
                }
        }
        Ok(())
    }
    fn handle_home(&mut self)
        -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor > 0 {
            self.rollin()?;
            self.input_cursor = 0;
        }
        Ok(())
    }
    fn handle_end(&mut self)
        -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor < self.input.len() {
            self.rollin()?;
            self.input_cursor = self.input.len();
        }
        Ok(())
    }
    fn handle_cancel(&mut self)
        -> LifeOrDeath {
        self.dismiss_notice()?;
        if !self.input.is_empty() {
            self.rollin()?;
            self.input.clear();
            self.input_cursor = 0;
        }
        Ok(())
    }
    fn handle_clear(&mut self)
        -> LifeOrDeath {
        // rollin, so that scrollback makes sense (on terminals that do it a
        // certain way)
        self.rollin()?;
        self.notice = None;
        self.roll_state = RollState::NothingShown;
        self.term.borrow_mut().clear_all_and_reset()?;
        Ok(())
    }
    fn handle_kill_to_end(&mut self)
        -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor < self.input.len() {
            self.rollin()?;
            self.input.replace_range(self.input_cursor.., "");
        }
        Ok(())
    }
    fn handle_delete_back(&mut self)
        -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor > 0 {
            self.rollin()?;
            let end_index = self.input_cursor;
            self.input_cursor -= 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible() {
                    self.input_cursor -= 1;
                }
            self.input.replace_range(self.input_cursor
                                     .. end_index,
                                     "");
        }
        Ok(())
    }
    fn handle_delete_fore(&mut self)
        -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor < self.input.len() {
            self.rollin()?;
            let start_index = self.input_cursor;
            self.input_cursor += 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible() {
                    self.input_cursor += 1;
                }
            self.input.replace_range(start_index ..
                                     self.input_cursor,
                                     "");
            self.input_cursor = start_index;
        }
        Ok(())
    }
    fn handle_delete_word(&mut self)
        -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor > 0 {
            self.rollin()?;
            let end_index = self.input_cursor;
            self.input_cursor -= 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible_or_space() {
                    self.input_cursor -= 1;
                }
            if self.input_cursor > 0 {
                while !self.input.is_char_boundary(self.input_cursor)
                    || self.cursor_on_invisible_or_nonspace() {
                        self.input_cursor -= 1;
                    }
                if !self.cursor_on_nonspace() {
                    self.input_cursor += 1;
                    while !self.input.is_char_boundary(self.input_cursor)
                        || self.cursor_on_invisible() {
                            self.input_cursor += 1;
                        }
                }
            }
            self.input.replace_range(self.input_cursor
                                     .. end_index,
                                     "");
        }
        Ok(())
    }
    fn handle_return(&mut self, tx: &mut tokio_mpsc::UnboundedSender<Response>)
    -> LifeOrDeath {
        self.rollin()?;
        self.notice = None;
        let mut input = String::new();
        swap(&mut input, &mut self.input);
        self.input_cursor = 0;
        tx.send(Response::Input(input))?;
        Ok(())
    }
    fn handle_finish(&mut self, tx: &mut tokio_mpsc::UnboundedSender<Response>)
    -> LifeOrDeath {
        if self.input.is_empty() {
            tx.send(Response::Finish)?;
        }
        else {
            self.rollin()?;
            self.input.clear();
            self.input_cursor = 0;
        }
        Ok(())
    }
    fn handle_input(&mut self, tx: &mut tokio_mpsc::UnboundedSender<Response>,
                    input: &str)
    -> LifeOrDeath {
        if !self.input_allowed { return Ok(()) }
        for ch in input.chars() {
            match ch {
                // Control-A (go to beginning of line)
                '\u{0001}' => self.handle_home()?,
                // Control-E (go to end of line)
                '\u{0005}' => self.handle_end()?,
                // Control-B (backward one char)
                '\u{0002}' => self.handle_left_arrow()?,
                // Control-F (forward one char)
                '\u{0006}' => self.handle_right_arrow()?,
                // Control-U (cancel input)
                '\u{0015}' => self.handle_cancel()?,
                // Control-K (kill line)
                '\u{000B}' => self.handle_kill_to_end()?,
                // Control-L (clear screen)
                '\u{000C}' => self.handle_clear()?,
                // Control-W (erase word)
                '\u{0017}' => self.handle_delete_word()?,
                // Tab
                '\t' => {
                    // TODO completion
                },
                // Control-C
                '\u{0003}' => {
                    tx.send(Response::Quit)?;
                },
                // Control-D
                '\u{0004}' => self.handle_finish(tx)?,
                // Control-T
                '\u{0014}' => {
                    tx.send(Response::Info)?;
                },
                // Control-Z
                '\u{001A}' => {
                    tx.send(Response::Suspend)?;
                },
                // Escape
                '\u{001B}' => {
                    tx.send(Response::Escape)?;
                },
                // Break (control-backslash)
                '\u{001C}' => {
                    tx.send(Response::Break)?;
                },
                // Control-X
                '\u{0018}' => {
                    tx.send(Response::Swap)?;
                },
                // Control-N (history next)
                '\u{000E}' => {
                    // TODO: history
                },
                // Control-P (history previous)
                '\u{0010}' => {
                    // TODO: history
                },
                // Enter/return
                '\n' | '\r' => self.handle_return(tx)?,
                // Backspace
                '\u{0008}' | '\u{007F}'
                    => self.handle_delete_back()?,
                // Unknown control character
                '\u{0000}' ..= '\u{001F}' | '\u{0080}' ..= '\u{009F}' => {
                    tx.send(Response::Unknown(ch as u8))?;
                },
                // Printable(?) text(??)
                _ => self.handle_char_input(ch)?,
            }
        }
        Ok(())
    }
    fn handle_event(&mut self, tx: &mut tokio_mpsc::UnboundedSender<Response>,
                    event: Event)
        -> LifeOrDeath {
        if !self.input_allowed { return Ok(()) }
        match event {
            Event::Resize(..) => self.rollin()?,
            Event::Mouse(..) => (),
            Event::Key(k) => {
                use crossterm::event::{KeyCode, KeyModifiers};
                if k.modifiers.contains(KeyModifiers::CONTROL) {
                    match k.code {
                        // Control-A (go to beginning of line)
                        KeyCode::Char('a')
                            => self.handle_home()?,
                        // Control-E (go to end of line)
                        KeyCode::Char('e')
                            => self.handle_end()?,
                        // Control-B (backward one char)
                        KeyCode::Char('b')
                            => self.handle_left_arrow()?,
                        // Control-F (forward one char)
                        KeyCode::Char('f')
                            => self.handle_right_arrow()?,
                        // Control-U (cancel input)
                        KeyCode::Char('u')
                            => self.handle_cancel()?,
                        // Control-K (kill line)
                        KeyCode::Char('k')
                            => self.handle_kill_to_end()?,
                        // Control-L (clear screen)
                        KeyCode::Char('l')
                            => self.handle_clear()?,
                        // Control-W (erase word)
                        KeyCode::Char('w')
                            => self.handle_delete_word()?,
                        // Control-C
                        KeyCode::Char('c') => {
                            tx.send(Response::Quit)?;
                        },
                        // Control-D
                        KeyCode::Char('d')
                            => self.handle_finish(tx)?,
                        // Control-T
                        KeyCode::Char('t') => {
                            tx.send(Response::Info)?;
                        },
                        // Control-Z
                        KeyCode::Char('z') => {
                            tx.send(Response::Suspend)?;
                        },
                        // Control-I (Tab)
                        KeyCode::Char('i') => {
                            // TODO completion
                        },
                        // Break (control-backslash)
                        KeyCode::Char('\\') => {
                            tx.send(Response::Break)?;
                        },
                        // Control-X
                        KeyCode::Char('x') => {
                            tx.send(Response::Swap)?;
                        },
                        // Control-N (history next)
                        KeyCode::Char('n') => {
                            // TODO: history
                        },
                        // Control-P (history previous)
                        KeyCode::Char('p') => {
                            // TODO: history
                        },
                        // Control-J/Control-M
                        KeyCode::Char('j') | KeyCode::Char('m')
                            => self.handle_return(tx)?,
                        // Unknown control character
                        KeyCode::Char(x) => {
                            if x >= '\u{0040}' && x <= '\u{007e}' {
                                tx.send(Response::Unknown((x as u8) & 0x1F))?;
                            }
                        },
                        _ => (),
                    }
                }
                else {
                    match k.code {
                        // Printable(?) text(??)
                        KeyCode::Char(ch) => {
                            if !ch.is_control() && ch != '\u{2028}'
                            && ch != '\u{2029}' {
                               self.handle_char_input(ch)?
                            }
                        },
                        KeyCode::Tab => {
                            // TODO completion
                        },
                        KeyCode::Esc =>
                            tx.send(Response::Escape)?,
                        KeyCode::Enter
                            => self.handle_return(tx)?,
                        KeyCode::Backspace
                            => self.handle_delete_back()?,
                        KeyCode::Delete
                            => self.handle_delete_fore()?,
                        KeyCode::Up => (), // TODO history
                        KeyCode::Down => (), // TODO history
                        KeyCode::Left
                            => self.handle_left_arrow()?,
                        KeyCode::Right
                            => self.handle_right_arrow()?,
                        KeyCode::Home
                            => self.handle_home()?,
                        KeyCode::End
                            => self.handle_end()?,
                        _ => (),
                    }
                }
            }
        }
        Ok(())
    }
    pub fn rollin(&mut self) -> LifeOrDeath {
        let rollback = match self.roll_state {
            RollState::NothingShown => 0,
            RollState::StatusShown(a) => a,
            RollState::EverythingShown(a, b) => a + b,
        };
        if self.input_allowed {
            self.term.borrow_mut().hide_cursor()?;
        }
        self.term.borrow_mut().carriage_return()?;
        if rollback > 0 {
            self.term.borrow_mut().move_cursor_up(rollback)?;
        }
        self.term.borrow_mut().clear_forward_and_reset()?;
        self.roll_state = RollState::NothingShown;
        Ok(())
    }
    pub fn rollout(&mut self) -> LifeOrDeath {
        let status_roll = match self.roll_state {
            RollState::NothingShown => {
                let break_count = if let Some(line) = self.status.as_ref() {
                    let mut cur_column = 0;
                    let mut break_count = 0;
                    self.output_line(line,
                                     &mut cur_column, &mut break_count,
                                     true, true, None)?;
                    self.term.borrow_mut().reset_attrs()?;
                    break_count
                }
                else { 0 };
                self.roll_state = RollState::StatusShown(break_count);
                break_count
            },
            RollState::StatusShown(x) => x,
            RollState::EverythingShown(..) => return Ok(()),
        };
        let mut cur_column = 0;
        let mut break_count = 0;
        if let Some((line, _)) = self.notice.as_ref() {
            self.output_line(line,
                             &mut cur_column,
                             &mut break_count, false, false, None)?;
        }
        else {
            if let Some(line) = self.prompt.as_ref() {
                self.output_line(line,
                                 &mut cur_column,
                                 &mut break_count, false, false, None)?;
            }
            let mut cursor_column = cur_column;
            let mut cursor_break = break_count;
            if !self.input.is_empty() {
                let line = Line::from_str(&self.input);
                self.output_line(&line,
                                 &mut cur_column,
                                 &mut break_count, false, true,
                                 Some((self.input_cursor,
                                       &mut cursor_column,
                                       &mut cursor_break)))?;
            }
            if cursor_column < cur_column {
                self.term.borrow_mut().move_cursor_left(cur_column - cursor_column)?;
            }
            else if cursor_column > cur_column {
                self.term.borrow_mut().move_cursor_right(cursor_column - cur_column)?;
            }
            if cursor_break != break_count {
                self.term.borrow_mut().move_cursor_up(break_count - cursor_break)?;
                break_count = cursor_break;
            }
            if self.input_allowed {
                self.term.borrow_mut().show_cursor()?;
            }
        }
        self.roll_state
            = RollState::EverythingShown(status_roll, break_count);
        self.term.borrow_mut().flush()?;
        Ok(())
    }
    fn cleanup(self) -> LifeOrDeath {
        RefCell::into_inner(self.term).cleanup()?;
        Ok(())
    }
}

/// This is the actual worker function we use when we're in "tty mode", that
/// is, we believe we have a terminal crossterm supports and NO PIPES.
fn tty_worker(req_tx: std_mpsc::Sender<Request>,
              rx: std_mpsc::Receiver<Request>,
              mut tx: tokio_mpsc::UnboundedSender<Response>)
    -> LifeOrDeath {
    let req_tx_clone = req_tx.clone();
    let (mut ded_tx, ded_rx) = std_mpsc::sync_channel(5);
    std::thread::Builder::new().name("Liso heartbeat thread".to_owned())
        .spawn(move || {
            let mut deadlines = Vec::with_capacity(4);
            loop {
                if deadlines.len() == 0 {
                    match ded_rx.recv() {
                        Ok(x) => deadlines.push(x),
                        Err(_) => break,
                    };
                }
                else {
                    let now = Instant::now();
                    while !deadlines.is_empty() && now >= deadlines[0] {
                        deadlines.remove(0);
                        match req_tx_clone.send(Request::Heartbeat) {
                            Ok(_) => break,
                            Err(_) => return,
                        }
                    }
                    if !deadlines.is_empty() {
                        use std::sync::mpsc::RecvTimeoutError;
                        let interval = deadlines[0] - now;
                        match ded_rx.recv_timeout(interval) {
                            Ok(x) => deadlines.push(x),
                            Err(RecvTimeoutError::Timeout) => (),
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                }
            }
        }).unwrap();
    crossterm::terminal::enable_raw_mode()?;
    let term = new_term(&req_tx)?;
    let mut state = TtyState {
        status: None, prompt: None, notice: None,
        roll_state: RollState::NothingShown, input_allowed: false,
        input: String::new(), input_cursor: 0, term: RefCell::new(term),
    };
    'outer: while let Ok(request) = rx.recv() {
        if let Request::Die = request { break }
        state.handle(&mut tx, &mut ded_tx, request)?;
        loop {
            use std_mpsc::TryRecvError;
            match rx.try_recv() {
                Ok(Request::Die) => break 'outer,
                Ok(request) => state.handle(&mut tx, &mut ded_tx, request)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'outer,
            }
        }
        state.rollout()?;
    }
    state.rollin()?;
    state.cleanup()?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

pub(crate) fn worker(req_tx: std_mpsc::Sender<Request>,
                     rx: std_mpsc::Receiver<Request>,
                     tx: tokio_mpsc::UnboundedSender<Response>)
-> LifeOrDeath {
    if !(std::io::stdout().is_tty() && std::io::stdin().is_tty())
    || std::env::var("TERM").as_ref().map(String::as_str) == Ok("dumb") {
        return pipe_worker(req_tx, rx, tx)
    }
    else {
        return tty_worker(req_tx, rx, tx)
    }
}
