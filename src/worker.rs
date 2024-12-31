//! Reads input, writes output. Communicates with your application by channels.

#![allow(unreachable_code)] // DELETE ME

use super::*;

use std::{
    cell::{RefCell, RefMut},
    io::BufRead,
    mem::swap,
    time::Instant,
};

use crossterm::tty::IsTty;
use unicode_width::UnicodeWidthChar;

/// This is the actual worker used when we're in "pipe mode". That means we
/// either have a dumb terminal or a piped stdin/stdout.
fn pipe_worker(
    req_tx: std_mpsc::Sender<Request>,
    rx: std_mpsc::Receiver<Request>,
    tx: tokio_mpsc::UnboundedSender<Response>,
) -> LifeOrDeath {
    std::thread::Builder::new()
        .name("Liso input thread".to_owned())
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
                        if req_tx.send(Request::RawInput(buf)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .unwrap();
    while let Ok(request) = rx.recv() {
        match request {
            #[cfg(feature = "wrap")]
            Request::Output(line) | Request::OutputWrapped(line) => {
                std::println!("{}", line.text);
            }
            #[cfg(not(feature = "wrap"))]
            Request::Output(line) => {
                std::println!("{}", line.text);
            }
            // stderr will not be captured if the pipe worker is being used.
            #[cfg(feature = "capture-stderr")]
            Request::StderrLine(_) => unreachable!(),
            Request::RawInput(x) => {
                if tx.send(Response::Input(x)).is_err() {
                    break;
                }
            }
            Request::Die => break,
            Request::Custom(x) => tx.send(Response::Custom(x))?,
            _ => (),
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RememberedOutput {
    output_line: Line,
    cursor_pos: Option<usize>,
    cursor_top: u32,
    cursor_left: u32,
}

struct TtyState {
    status: Option<Line>,
    prompt: Option<Line>,
    notice: Option<(Line, Instant)>,
    input: String,
    clipboard: String,
    input_cursor: usize,
    input_allowed: bool,
    remembered_output: Option<RememberedOutput>,
    rollout_needed: bool,
    term: RefCell<Box<dyn Term>>,
    #[cfg(feature = "completion")]
    own_output: Output,
    #[cfg(feature = "history")]
    history: Arc<RwLock<History>>,
    /// `None` = editing the "new" line. `Some(i)` = editing a line that
    /// originated in history.
    #[cfg(feature = "history")]
    cur_history_index: Option<usize>,
    /// Input that was on the "new" line, not originally part of history.
    #[cfg(feature = "history")]
    orphaned_new_input: Option<String>,
    /// What the currently selected history line looked like before any edits
    /// we might have performed. (Used when history is bumped, to try to find
    /// our place again.)
    #[cfg(feature = "history")]
    history_original_line: Option<String>,
    #[cfg(feature = "completion")]
    completor: Option<Box<dyn Completor>>,
    #[cfg(feature = "completion")]
    consecutive_completion_presses: u32,
}

impl TtyState {
    /// Output a Line, followed by a single linebreak.
    fn output_line(&self, line: &Line) -> LifeOrDeath {
        let mut term = self.term.borrow_mut();
        let term_width = term.get_width();
        let mut cur_column = 0;
        for element in line.elements.iter() {
            term.set_attrs(element.style, element.fg, element.bg)?;
            let text = &line.text[element.start..element.end];
            let mut cur = 0;
            for (idx, ch) in text.char_indices() {
                let char_width =
                    UnicodeWidthChar::width(ch).unwrap_or(0) as u32;
                if (char_width > 0 && cur_column >= term_width) || ch == '\n' {
                    if cur != idx {
                        term.print(&text[cur..idx])?;
                    }
                    cur = idx;
                    if ch == '\n' {
                        cur += 1
                    }
                    if cur_column < term_width {
                        if term.cur_style().contains(Style::INVERSE)
                            || term.cur_style().contains(Style::UNDERLINE)
                            || element.bg.is_some()
                        {
                            term.print_spaces(
                                (term_width - cur_column) as usize,
                            )?;
                        } else {
                            term.clear_to_end_of_line()?;
                        }
                    }
                    term.newline()?;
                    cur_column = 0;
                }
                cur_column += char_width;
            }
            if cur != text.len() {
                term.print(&text[cur..])?;
            }
        }
        let trailit = match line.elements.last() {
            None => false,
            Some(el) => {
                el.style.contains(Style::INVERSE)
                    || el.style.contains(Style::UNDERLINE)
                    || el.bg.is_some()
            }
        };
        if trailit && cur_column < term_width {
            term.print_spaces((term_width - cur_column) as usize)?;
        }
        term.set_attrs(Style::empty(), None, None)?;
        if cur_column != term_width {
            term.clear_to_end_of_line()?;
        }
        term.newline()?;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn maybe_report(
        &self,
        index: usize,
        cur_column: u32,
        cur_breaks: u32,
        cursor_pos: Option<usize>,
        out_column: &mut Option<u32>,
        out_breaks: &mut Option<u32>,
        term_width: u32,
    ) {
        if let Some(cursor_pos) = cursor_pos {
            if index == cursor_pos {
                if cur_column >= term_width {
                    *out_column = Some(term_width);
                    *out_breaks = Some(cur_breaks);
                } else {
                    *out_column = Some(cur_column);
                    *out_breaks = Some(cur_breaks);
                }
            }
        }
    }
    fn reconcile_cursors(
        &self,
        term: &mut RefMut<'_, Box<dyn Term>>,
        term_width: u32,
        real_column: &mut u32,
        real_breaks: &mut u32,
        mut cur_column: u32,
        cur_breaks: u32,
    ) -> LifeOrDeath {
        cur_column = cur_column.min(term_width);
        *real_column = (*real_column).min(term_width);
        let cur_shown_column = cur_column.min(term_width - 1);
        let real_shown_column = (*real_column).min(term_width - 1);
        if *real_breaks != cur_breaks {
            if *real_breaks < cur_breaks {
                term.move_cursor_down(cur_breaks - *real_breaks)?;
            } else {
                debug_assert!(*real_breaks > cur_breaks);
                term.move_cursor_up(*real_breaks - cur_breaks)?;
            }
            *real_breaks = cur_breaks;
        }
        if real_shown_column != cur_shown_column {
            if real_shown_column < cur_shown_column {
                term.move_cursor_right(cur_shown_column - real_shown_column)?;
            } else {
                debug_assert!(real_shown_column > cur_shown_column);
                term.move_cursor_left(real_shown_column - cur_shown_column)?;
            }
            *real_column = cur_column;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn output_char(
        &self,
        term: &mut RefMut<'_, Box<dyn Term>>,
        term_width: u32,
        cur_attr: &mut (Style, Option<Color>, Option<Color>),
        lc: LineChar,
        cur_column: &mut u32,
        cur_breaks: &mut u32,
        implied_newline: &mut bool,
    ) -> LifeOrDeath {
        if (lc.style, lc.fg, lc.bg) != *cur_attr {
            term.set_attrs(lc.style, lc.fg, lc.bg)?;
            *cur_attr = (lc.style, lc.fg, lc.bg);
        }
        let ch = lc.ch;
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0) as u32;
        if ch != '\n' {
            term.print_char(ch)?;
            *cur_column += char_width;
            *implied_newline = false;
        } else if ch == '\n' && *implied_newline {
            *implied_newline = false;
            return Ok(());
        }
        if (char_width > 0 && *cur_column >= term_width) || ch == '\n' {
            if *cur_column < term_width {
                if cur_attr.0.contains(Style::INVERSE)
                    || cur_attr.0.contains(Style::UNDERLINE)
                    || cur_attr.2.is_some()
                {
                    term.print_spaces((term_width - *cur_column) as usize)?;
                } else {
                    term.clear_to_end_of_line()?;
                }
            }
            term.newline()?;
            *implied_newline = ch != '\n';
            *cur_breaks += 1;
            *cur_column = 0;
        }
        Ok(())
    }
    fn sim_output_char(
        &self,
        term_width: u32,
        lc: LineChar,
        cur_column: &mut u32,
        cur_breaks: &mut u32,
        implied_newline: &mut bool,
    ) -> LifeOrDeath {
        let ch = lc.ch;
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0) as u32;
        if ch != '\n' {
            *cur_column += char_width;
            *implied_newline = false;
        } else if ch == '\n' && *implied_newline {
            *implied_newline = false;
            return Ok(());
        }
        if (char_width > 0 && *cur_column >= term_width) || ch == '\n' {
            *cur_breaks += 1;
            *cur_column = 0;
            *implied_newline = ch != '\n';
        }
        Ok(())
    }
    /// Bit weird. Also outputs a line, but remembers the most recent output
    /// (via the `remembered_output` member) and tries to update it with as
    /// little unnecessary cursor movement as possible.
    ///
    /// - `new_line`: The line to output.
    /// - `cursor_pos`: If present, the cursor will be moved to the terminal
    ///   position that corresponds to the given byte position in the line's
    ///   text. If absent, the cursor will be wherever it wants to be.
    /// - `break_after`: If true, we will output one last linebreak at the end
    ///   of the line, iff the line didn't end on a newline.
    /// - `endfill`: If true, we will ensure that the current background color
    ///   and/or inversion is padded out to the end of the line. (This must
    ///   always be true when `break_after` is true.)
    pub fn output_line_changes(
        &mut self,
        new_line: &Line,
        cursor_pos: Option<usize>,
        break_after: bool,
        endfill: bool,
    ) -> LifeOrDeath {
        if break_after {
            debug_assert!(endfill);
        }
        let mut real_column;
        let mut real_breaks;
        if let Some(rem) = self.remembered_output.as_ref() {
            if new_line == &rem.output_line && cursor_pos == rem.cursor_pos {
                // No change required
                return Ok(());
            }
            real_column = rem.cursor_left;
            real_breaks = rem.cursor_top;
        } else {
            real_column = 0;
            real_breaks = 0;
        }
        let mut term = self.term.borrow_mut();
        let term_width = term.get_width();
        let mut new_chars = new_line.chars();
        let mut old_chars = self
            .remembered_output
            .as_ref()
            .map(|x| x.output_line.chars());
        let mut cur_attr = (Style::empty(), None, None);
        let mut cur_column = 0;
        let mut cur_breaks = 0;
        let mut endfill_redundant = false;
        let mut output_cursor_top = None;
        let mut output_cursor_left = None;
        let mut implied_newline = false;
        let ended_simultaneously = loop {
            match (old_chars.as_mut().and_then(|x| x.next()), new_chars.next())
            {
                (Some(a), Some(b)) => {
                    self.maybe_report(
                        b.index,
                        cur_column,
                        cur_breaks,
                        cursor_pos,
                        &mut output_cursor_left,
                        &mut output_cursor_top,
                        term_width,
                    );
                    if a == b {
                        self.sim_output_char(
                            term_width,
                            b,
                            &mut cur_column,
                            &mut cur_breaks,
                            &mut implied_newline,
                        )?;
                        continue;
                    }
                    // we have a difference! Let the real cursor catch up
                    self.reconcile_cursors(
                        &mut term,
                        term_width,
                        &mut real_column,
                        &mut real_breaks,
                        cur_column,
                        cur_breaks,
                    )?;
                    if (a.ch == '\n') != (b.ch == '\n') {
                        // Simpler at this point just to clear everything.
                        term.clear_forward_and_reset()?;
                        cur_attr = (Style::empty(), None, None);
                        old_chars = None;
                    }
                    self.output_char(
                        &mut term,
                        term_width,
                        &mut cur_attr,
                        b,
                        &mut cur_column,
                        &mut cur_breaks,
                        &mut implied_newline,
                    )?;
                    real_column = cur_column;
                    real_breaks = cur_breaks;
                    endfill_redundant = a.endfills_same_as(&b);
                }
                (None, Some(b)) => {
                    self.maybe_report(
                        b.index,
                        cur_column,
                        cur_breaks,
                        cursor_pos,
                        &mut output_cursor_left,
                        &mut output_cursor_top,
                        term_width,
                    );
                    self.reconcile_cursors(
                        &mut term,
                        term_width,
                        &mut real_column,
                        &mut real_breaks,
                        cur_column,
                        cur_breaks,
                    )?;
                    self.output_char(
                        &mut term,
                        term_width,
                        &mut cur_attr,
                        b,
                        &mut cur_column,
                        &mut cur_breaks,
                        &mut implied_newline,
                    )?;
                    real_column = cur_column;
                    real_breaks = cur_breaks;
                    endfill_redundant = false;
                    break false;
                }
                (a, None) => {
                    if a.is_some() {
                        self.reconcile_cursors(
                            &mut term,
                            term_width,
                            &mut real_column,
                            &mut real_breaks,
                            cur_column,
                            cur_breaks,
                        )?;
                        if real_column == term_width
                            && cur_column + 1 == term_width
                        {
                            term.print_spaces(1)?;
                        } else if cur_column == term_width {
                            term.newline()?;
                            term.clear_forward_and_reset()?;
                            real_column = 0;
                            real_breaks += 1;
                            self.reconcile_cursors(
                                &mut term,
                                term_width,
                                &mut real_column,
                                &mut real_breaks,
                                cur_column,
                                cur_breaks,
                            )?;
                        } else {
                            term.clear_forward_and_reset()?;
                        }
                        endfill_redundant = false;
                    }
                    break a.is_none();
                }
            }
        };
        for b in new_chars {
            debug_assert!(!ended_simultaneously);
            self.maybe_report(
                b.index,
                cur_column,
                cur_breaks,
                cursor_pos,
                &mut output_cursor_left,
                &mut output_cursor_top,
                term_width,
            );
            self.reconcile_cursors(
                &mut term,
                term_width,
                &mut real_column,
                &mut real_breaks,
                cur_column,
                cur_breaks,
            )?;
            self.output_char(
                &mut term,
                term_width,
                &mut cur_attr,
                b,
                &mut cur_column,
                &mut cur_breaks,
                &mut implied_newline,
            )?;
            real_column = cur_column;
            real_breaks = cur_breaks;
        }
        self.maybe_report(
            new_line.text.len(),
            cur_column,
            cur_breaks,
            cursor_pos,
            &mut output_cursor_left,
            &mut output_cursor_top,
            term_width,
        );
        if !ended_simultaneously || !endfill_redundant {
            let trailit = endfill
                && match new_line.elements.last() {
                    None => false,
                    Some(el) => {
                        el.style.contains(Style::INVERSE)
                            || el.style.contains(Style::UNDERLINE)
                            || el.bg.is_some()
                    }
                };
            if trailit && cur_column < term_width {
                self.reconcile_cursors(
                    &mut term,
                    term_width,
                    &mut real_column,
                    &mut real_breaks,
                    cur_column,
                    cur_breaks,
                )?;
                let last = new_line.elements.last().unwrap();
                term.set_attrs(last.style, last.fg, last.bg)?;
                term.print_spaces((term_width - cur_column) as usize)?;
                cur_column = term_width;
                real_column = cur_column;
            }
        }
        term.set_attrs(Style::empty(), None, None)?;
        if break_after && !implied_newline {
            if !endfill && cur_column != term_width {
                term.clear_to_end_of_line()?;
            }
            term.newline()?;
            cur_column = 0;
            cur_breaks += 1;
        }
        let cursor_left = output_cursor_left.unwrap_or(cur_column);
        let cursor_top = output_cursor_top.unwrap_or(cur_breaks);
        self.reconcile_cursors(
            &mut term,
            term_width,
            &mut real_column,
            &mut real_breaks,
            cursor_left,
            cursor_top,
        )?;
        if cursor_top > cur_breaks {
            // this should only happen if the cursor went to the next line, but
            // no chars did
            assert_eq!(cur_breaks + 1, cursor_top);
            term.newline()?;
            if endfill {
                term.print_spaces(term_width as usize)?;
                term.carriage_return()?;
            }
        }
        self.remembered_output = Some(RememberedOutput {
            output_line: new_line.clone(),
            cursor_pos,
            cursor_left,
            cursor_top,
        });
        Ok(())
    }
    pub fn handle(
        &mut self,
        tx: &mut tokio_mpsc::UnboundedSender<Response>,
        ded_tx: &mut std_mpsc::SyncSender<Instant>,
        request: Request,
    ) -> LifeOrDeath {
        match request {
            Request::Output(line) | Request::OutputEcho(line) => {
                self.rollin()?;
                self.output_line(&line)?;
                self.term.borrow_mut().reset_attrs()?;
            }
            #[cfg(feature = "capture-stderr")]
            Request::StderrLine(mut text) => {
                if text.ends_with("\r") {
                    text.pop();
                }
                // TODO: custom decorators?
                self.rollin()?;
                self.output_line(&liso!(fg = red, bold, "E: ", -bold, text))?;
                self.term.borrow_mut().reset_attrs()?;
            }
            #[cfg(feature = "wrap")]
            Request::OutputWrapped(mut line) => {
                self.rollin()?;
                line.wrap_to_width(self.term.borrow_mut().get_width() as usize);
                self.output_line(&line)?;
                self.term.borrow_mut().reset_attrs()?;
            }
            Request::SuspendAndRun(mut wat) => {
                self.rollin()?;
                self.remembered_output = None;
                self.term.borrow_mut().suspend()?;
                wat();
                self.term.borrow_mut().unsuspend()?;
            }
            Request::Status(line) => {
                if self.status != line {
                    self.rollout_needed = true;
                    self.status = line;
                }
            }
            Request::Notice(line, duration) => {
                self.show_notice(line, duration, ded_tx)?;
            }
            Request::Prompt {
                line,
                input_allowed,
                clear_input,
            } => {
                if self.prompt != line
                    || (clear_input && !self.input.is_empty())
                {
                    self.rollout_needed = true;
                    self.prompt = line;
                    self.input_allowed = input_allowed;
                    if clear_input {
                        self.input.clear();
                        #[cfg(feature = "history")]
                        {
                            self.cur_history_index = None;
                            self.orphaned_new_input = None;
                            self.history_original_line = None;
                        }
                    }
                }
            }
            Request::Bell => self.term.borrow_mut().bell()?,
            Request::RawInput(input) => {
                self.handle_input(tx, &input, ded_tx)?
            }
            Request::CrosstermEvent(event) => {
                self.handle_event(tx, event, ded_tx)?
            }
            Request::Die => return Ok(()),
            Request::Heartbeat => {
                if let Some((_, deadline)) = self.notice {
                    if Instant::now() >= deadline {
                        self.rollout_needed = true;
                        self.notice = None;
                    }
                }
            }
            Request::Custom(x) => tx.send(Response::Custom(x))?,
            #[cfg(feature = "history")]
            Request::BumpHistory => {
                if self.cur_history_index.is_some() {
                    let history = self.history.read().unwrap();
                    let lines = history.get_lines();
                    self.cur_history_index = None;
                    if let Some(history_original_line) =
                        self.history_original_line.as_ref()
                    {
                        for (i, x) in lines.iter().enumerate().rev() {
                            if x == history_original_line {
                                self.cur_history_index = Some(i);
                                break;
                            }
                        }
                    }
                    match self.cur_history_index {
                        None => {
                            self.orphaned_new_input = None;
                            self.history_original_line = None;
                        }
                        Some(x) => {
                            self.history_original_line =
                                Some(lines[x].clone());
                        }
                    }
                }
            }
            #[cfg(feature = "completion")]
            Request::SetCompletor(completor) => self.completor = completor,
        }
        Ok(())
    }
    fn cursor_on<F>(&self, f: F) -> bool
    where
        F: FnOnce(char) -> bool,
    {
        self.input_cursor < self.input.len()
            && f(self.input[self.input_cursor..].chars().next().unwrap())
    }
    /// returns true if the cursor is currently "on" an invisible character
    ///
    /// (won't return true for the first character)
    fn cursor_on_invisible(&self) -> bool {
        self.input_cursor > 0
            && self.cursor_on(|x| UnicodeWidthChar::width(x).unwrap_or(0) == 0)
    }
    /// returns true if the cursor is currently "on" an invisible character OR
    /// a space character
    ///
    /// (won't return true for the first character)
    fn cursor_on_invisible_or_space(&self) -> bool {
        self.input_cursor > 0
            && self.cursor_on(|x| {
                x.is_whitespace()
                    || UnicodeWidthChar::width(x).unwrap_or(0) == 0
            })
    }
    /// returns true if the cursor is currently "on" an invisible character OR
    /// a nonspace character
    ///
    /// (won't return true for the first character)
    fn cursor_on_invisible_or_nonspace(&self) -> bool {
        self.input_cursor > 0
            && self.cursor_on(|x| {
                !x.is_whitespace()
                    || UnicodeWidthChar::width(x).unwrap_or(0) == 0
            })
    }
    /// returns true if the cursor is currently "on" a nonspace character
    ///
    /// (MIGHT return true for the first character)
    fn cursor_on_nonspace(&self) -> bool {
        self.cursor_on(|x| !x.is_whitespace())
    }
    fn dismiss_notice(&mut self) -> LifeOrDeath {
        if self.notice.is_some() {
            self.rollout_needed = true;
            self.notice = None;
        }
        Ok(())
    }
    fn handle_char_input(&mut self, ch: char) -> LifeOrDeath {
        self.rollout_needed = true;
        self.notice = None;
        self.input.insert(self.input_cursor, ch);
        self.input_cursor += 1;
        while !self.input.is_char_boundary(self.input_cursor) {
            self.input_cursor += 1;
        }
        Ok(())
    }
    fn handle_right_arrow(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor < self.input.len() {
            self.rollout_needed = true;
            self.input_cursor += 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible()
            {
                self.input_cursor += 1;
            }
        }
        Ok(())
    }
    fn handle_left_arrow(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor > 0 {
            self.rollout_needed = true;
            self.input_cursor -= 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible()
            {
                self.input_cursor -= 1;
            }
        }
        Ok(())
    }
    fn handle_home(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor > 0 {
            self.rollout_needed = true;
            self.input_cursor = 0;
        }
        Ok(())
    }
    fn handle_end(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor < self.input.len() {
            self.rollout_needed = true;
            self.input_cursor = self.input.len();
        }
        Ok(())
    }
    fn handle_discard(
        &mut self,
        tx: &mut tokio_mpsc::UnboundedSender<Response>,
    ) -> LifeOrDeath {
        self.dismiss_notice()?;
        let mut input = String::new();
        swap(&mut input, &mut self.input);
        #[cfg(feature = "history")]
        {
            self.cur_history_index = None;
            self.orphaned_new_input = None;
            self.history_original_line = None;
        }
        let was_empty = input.is_empty();
        tx.send(Response::Discarded(input))?;
        if !was_empty {
            self.rollout_needed = true;
            self.input.clear();
            self.input_cursor = 0;
        }
        Ok(())
    }
    fn handle_clear(&mut self) -> LifeOrDeath {
        // rollin, so that scrollback makes sense (on terminals that do it a
        // certain way)
        self.rollin()?;
        self.rollout_needed = true;
        self.notice = None;
        self.term.borrow_mut().clear_all_and_reset()?;
        Ok(())
    }
    fn handle_kill_to_end(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor < self.input.len() {
            self.rollout_needed = true;
            self.clipboard = self.input[self.input_cursor..].to_string();
            self.input.replace_range(self.input_cursor.., "");
        }
        Ok(())
    }
    fn handle_kill_to_start(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor > 0 {
            self.rollout_needed = true;
            self.clipboard = self.input[..self.input_cursor].to_string();
            self.input.replace_range(..self.input_cursor, "");
            self.input_cursor = 0;
        }
        Ok(())
    }
    fn handle_yank(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        self.rollout_needed = true;
        self.input.replace_range(
            self.input_cursor..self.input_cursor,
            &self.clipboard,
        );
        self.input_cursor += self.clipboard.len();
        Ok(())
    }
    fn handle_delete_back(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor > 0 {
            self.rollout_needed = true;
            let end_index = self.input_cursor;
            self.input_cursor -= 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible()
            {
                self.input_cursor -= 1;
            }
            self.input.replace_range(self.input_cursor..end_index, "");
        }
        Ok(())
    }
    fn handle_delete_fore(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor < self.input.len() {
            self.rollout_needed = true;
            let start_index = self.input_cursor;
            self.input_cursor += 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible()
            {
                self.input_cursor += 1;
            }
            self.input.replace_range(start_index..self.input_cursor, "");
            self.input_cursor = start_index;
        }
        Ok(())
    }
    fn handle_delete_word(&mut self) -> LifeOrDeath {
        self.dismiss_notice()?;
        if self.input_cursor > 0 {
            self.rollout_needed = true;
            let end_index = self.input_cursor;
            self.input_cursor -= 1;
            while !self.input.is_char_boundary(self.input_cursor)
                || self.cursor_on_invisible_or_space()
            {
                self.input_cursor -= 1;
            }
            if self.input_cursor > 0 {
                while !self.input.is_char_boundary(self.input_cursor)
                    || self.cursor_on_invisible_or_nonspace()
                {
                    self.input_cursor -= 1;
                }
                if !self.cursor_on_nonspace() {
                    self.input_cursor += 1;
                    while !self.input.is_char_boundary(self.input_cursor)
                        || self.cursor_on_invisible()
                    {
                        self.input_cursor += 1;
                    }
                }
            }
            self.input.replace_range(self.input_cursor..end_index, "");
        }
        Ok(())
    }
    fn handle_return(
        &mut self,
        tx: &mut tokio_mpsc::UnboundedSender<Response>,
        _ded_tx: &mut std_mpsc::SyncSender<Instant>,
    ) -> LifeOrDeath {
        self.rollout_needed = true;
        self.notice = None;
        let mut input = String::new();
        swap(&mut input, &mut self.input);
        self.input_cursor = 0;
        #[cfg(feature = "history")]
        {
            self.cur_history_index = None;
            self.orphaned_new_input = None;
            self.history_original_line = None;
            let mut lock = self.history.write().unwrap();
            if let Err(e) = lock.add_line(input.clone()) {
                // TODO: make localizable
                let e = format!("Unable to write history: {}", e);
                drop(lock);
                self.show_notice(
                    liso!(inverse, e),
                    Duration::from_secs(3),
                    _ded_tx,
                )?;
            }
        }
        tx.send(Response::Input(input))?;
        Ok(())
    }
    fn handle_finish(
        &mut self,
        tx: &mut tokio_mpsc::UnboundedSender<Response>,
    ) -> LifeOrDeath {
        if self.input.is_empty() {
            tx.send(Response::Finish)?;
        } else {
            self.rollout_needed = true;
            self.input.clear();
            #[cfg(feature = "history")]
            {
                self.cur_history_index = None;
                self.orphaned_new_input = None;
                self.history_original_line = None;
            }
            self.input_cursor = 0;
        }
        Ok(())
    }
    fn handle_input(
        &mut self,
        tx: &mut tokio_mpsc::UnboundedSender<Response>,
        input: &str,
        ded_tx: &mut std_mpsc::SyncSender<Instant>,
    ) -> LifeOrDeath {
        if !self.input_allowed {
            return Ok(());
        }
        for ch in input.chars() {
            #[cfg(feature = "completion")]
            if ch == '\t' {
                self.consecutive_completion_presses =
                    self.consecutive_completion_presses.saturating_add(1);
            } else {
                self.consecutive_completion_presses = 0;
            }
            match ch {
                // Control-A (go to beginning of line)
                '\u{0001}' => self.handle_home()?,
                // Control-B (backward one char)
                '\u{0002}' => self.handle_left_arrow()?,
                // Control-C
                '\u{0003}' => tx.send(Response::Quit)?,
                // Control-D
                '\u{0004}' => self.handle_finish(tx)?,
                // Control-E (go to end of line)
                '\u{0005}' => self.handle_end()?,
                // Control-F (forward one char)
                '\u{0006}' => self.handle_right_arrow()?,
                // Control-G (discard input)
                '\u{0007}' => self.handle_discard(tx)?,
                // Control-K (kill line after cursor)
                '\u{000B}' => self.handle_kill_to_end()?,
                // Control-L (clear screen)
                '\u{000C}' => self.handle_clear()?,
                // Control-N (history next)
                #[cfg(feature = "history")]
                '\u{000E}' => self.history_next()?,
                // Control-P (history previous)
                #[cfg(feature = "history")]
                '\u{0010}' => self.history_prev()?,
                // Control-T
                '\u{0014}' => tx.send(Response::Info)?,
                // Control-U (kill line before cursor)
                '\u{0015}' => self.handle_kill_to_start()?,
                // Control-W (erase word)
                '\u{0017}' => self.handle_delete_word()?,
                // Control-X
                '\u{0018}' => tx.send(Response::Swap)?,
                // Control-Y (yank)
                '\u{0019}' => self.handle_yank()?,
                #[cfg(unix)]
                // Control-Z
                '\u{001A}' => self.handle_suspend()?,
                // Tab
                '\t' => self.handle_completion()?,
                // Escape
                '\u{001B}' => {
                    tx.send(Response::Escape)?;
                }
                // Break (control-backslash)
                '\u{001C}' => {
                    tx.send(Response::Break)?;
                }
                // Enter/return
                '\n' | '\r' => self.handle_return(tx, ded_tx)?,
                // Backspace
                '\u{0008}' | '\u{007F}' => self.handle_delete_back()?,
                // Unknown control character
                '\u{0000}'..='\u{001F}' | '\u{0080}'..='\u{009F}' => {
                    tx.send(Response::Unknown(ch as u8))?;
                }
                // Printable(?) text(??)
                _ => self.handle_char_input(ch)?,
            }
        }
        Ok(())
    }
    fn handle_event(
        &mut self,
        tx: &mut tokio_mpsc::UnboundedSender<Response>,
        event: Event,
        ded_tx: &mut std_mpsc::SyncSender<Instant>,
    ) -> LifeOrDeath {
        if !self.input_allowed {
            return Ok(());
        }
        match event {
            Event::Resize(..) => self.rollin()?,
            Event::Mouse(..) => (),
            Event::Key(k) => {
                use crossterm::event::{KeyCode, KeyModifiers};
                if k.modifiers.contains(KeyModifiers::CONTROL) {
                    #[cfg(feature = "completion")]
                    if k.code == KeyCode::Char('i') {
                        self.consecutive_completion_presses = self
                            .consecutive_completion_presses
                            .saturating_add(1);
                    } else {
                        self.consecutive_completion_presses = 0;
                    }
                    match k.code {
                        // Control-A (go to beginning of line)
                        KeyCode::Char('a') => self.handle_home()?,
                        // Control-B (backward one char)
                        KeyCode::Char('b') => self.handle_left_arrow()?,
                        // Control-C
                        KeyCode::Char('c') => tx.send(Response::Quit)?,
                        // Control-D
                        KeyCode::Char('d') => self.handle_finish(tx)?,
                        // Control-E (go to end of line)
                        KeyCode::Char('e') => self.handle_end()?,
                        // Control-F (forward one char)
                        KeyCode::Char('f') => self.handle_right_arrow()?,
                        // Control-G (discard input)
                        KeyCode::Char('g') => self.handle_discard(tx)?,
                        // Control-K (kill line after cursor)
                        KeyCode::Char('k') => self.handle_kill_to_end()?,
                        // Control-L (clear screen)
                        KeyCode::Char('l') => self.handle_clear()?,
                        // Control-N (history next)
                        #[cfg(feature = "history")]
                        KeyCode::Char('n') => self.history_next()?,
                        // Control-P (history previous)
                        #[cfg(feature = "history")]
                        KeyCode::Char('p') => self.history_prev()?,
                        // Control-T
                        KeyCode::Char('t') => tx.send(Response::Info)?,
                        // Control-U (kill line before cursor)
                        KeyCode::Char('u') => self.handle_kill_to_start()?,
                        // Control-W (erase word)
                        KeyCode::Char('w') => self.handle_delete_word()?,
                        // Control-X
                        KeyCode::Char('x') => tx.send(Response::Swap)?,
                        // Control-Y (yank)
                        KeyCode::Char('y') => self.handle_yank()?,
                        #[cfg(unix)]
                        // Control-Z
                        KeyCode::Char('z') => self.handle_suspend()?,
                        // Break (control-backslash)
                        KeyCode::Char('\\') => {
                            tx.send(Response::Break)?;
                        }
                        // Control-I (Tab)
                        KeyCode::Char('i') => self.handle_completion()?,
                        // Control-J/Control-M = return
                        KeyCode::Char('j') | KeyCode::Char('m') => {
                            self.handle_return(tx, ded_tx)?
                        }
                        // Unknown control character
                        KeyCode::Char(x) => {
                            if ('\u{0040}'..='\u{007e}').contains(&x) {
                                tx.send(Response::Unknown((x as u8) & 0x1F))?;
                            }
                        }
                        _ => (),
                    }
                } else {
                    #[cfg(feature = "completion")]
                    if k.code == KeyCode::Tab {
                        self.consecutive_completion_presses = self
                            .consecutive_completion_presses
                            .saturating_add(1);
                    } else {
                        self.consecutive_completion_presses = 0;
                    }
                    match k.code {
                        // Printable(?) text(??)
                        KeyCode::Char(ch) => {
                            if !ch.is_control()
                                && ch != '\u{2028}'
                                && ch != '\u{2029}'
                            {
                                self.handle_char_input(ch)?
                            }
                        }
                        KeyCode::Tab => self.handle_completion()?,
                        KeyCode::Esc => tx.send(Response::Escape)?,
                        KeyCode::Enter => self.handle_return(tx, ded_tx)?,
                        KeyCode::Backspace => self.handle_delete_back()?,
                        KeyCode::Delete => self.handle_delete_fore()?,
                        #[cfg(feature = "history")]
                        KeyCode::Up => self.history_prev()?,
                        #[cfg(feature = "history")]
                        KeyCode::Down => self.history_next()?,
                        KeyCode::Left => self.handle_left_arrow()?,
                        KeyCode::Right => self.handle_right_arrow()?,
                        KeyCode::Home => self.handle_home()?,
                        KeyCode::End => self.handle_end()?,
                        _ => (),
                    }
                }
            }
            Event::FocusGained | Event::FocusLost => (),
            Event::Paste(_) => {
                unreachable!("we don't turn bracketed paste on so we should never get this event")
            }
        }
        Ok(())
    }
    fn show_notice(
        &mut self,
        line: Line,
        duration: Duration,
        ded_tx: &mut std_mpsc::SyncSender<Instant>,
    ) -> LifeOrDeath {
        self.rollout_needed = true;
        let deadline = Instant::now() + duration;
        self.notice = Some((line, deadline));
        ded_tx.send(deadline)?;
        Ok(())
    }
    #[cfg(feature = "history")]
    fn history_prev(&mut self) -> LifeOrDeath {
        let history = self.history.read().unwrap();
        let prev_history_index = match self.cur_history_index {
            None => history.get_lines().len().checked_sub(1),
            Some(x) if x > 0 => Some(x - 1),
            _ => None,
        };
        match prev_history_index {
            None => {
                let mut term = self.term.borrow_mut();
                term.bell()?;
            }
            Some(prev_history_index) => {
                self.rollout_needed = true;
                let mut historical_line =
                    history.get_lines()[prev_history_index].clone();
                swap(&mut historical_line, &mut self.input);
                if self.orphaned_new_input.is_none() {
                    self.orphaned_new_input = Some(historical_line);
                }
                self.input_cursor = self.input.len();
                self.cur_history_index = Some(prev_history_index);
            }
        }
        self.history_original_line = self
            .cur_history_index
            .map(|i| history.get_lines()[i].clone());
        Ok(())
    }
    #[cfg(feature = "history")]
    fn history_next(&mut self) -> LifeOrDeath {
        let history = self.history.read().unwrap();
        match self.cur_history_index {
            None => {
                let mut term = self.term.borrow_mut();
                term.bell()?;
            }
            Some(x) if x + 1 == history.get_lines().len() => {
                assert!(self.orphaned_new_input.is_some());
                self.rollout_needed = true;
                self.input = self.orphaned_new_input.take().unwrap();
                self.input_cursor = self.input.len();
                self.cur_history_index = None;
            }
            Some(x) => {
                let next_history_index = x + 1;
                self.rollout_needed = true;
                let mut historical_line =
                    history.get_lines()[next_history_index].clone();
                swap(&mut historical_line, &mut self.input);
                if self.orphaned_new_input.is_none() {
                    self.orphaned_new_input = Some(historical_line);
                }
                self.input_cursor = self.input.len();
                self.cur_history_index = Some(next_history_index);
            }
        }
        self.history_original_line = self
            .cur_history_index
            .map(|i| history.get_lines()[i].clone());
        Ok(())
    }
    #[cfg(unix)]
    fn handle_suspend(&mut self) -> LifeOrDeath {
        self.rollout()?;
        self.remembered_output = None;
        let mut term = self.term.borrow_mut();
        term.set_attrs(Style::PLAIN, None, None)?;
        term.suspend()?;
        std::println!("^Z");
        unix_util::sigstop_ourselves();
        term.unsuspend()?;
        self.rollout_needed = true;
        Ok(())
    }
    fn handle_completion(&mut self) -> LifeOrDeath {
        #[cfg(feature = "completion")]
        {
            if let Some(completor) = self.completor.as_mut() {
                match completor.complete(
                    &self.own_output,
                    &self.input,
                    self.input_cursor,
                    NonZeroU32::new(self.consecutive_completion_presses)
                        .unwrap(),
                ) {
                    // DO NOT beep, print a notice, etc. They are supposed to
                    // have done that themselves.
                    None => (),
                    Some(Completion::InsertAtCursor { text }) => {
                        if !text.is_empty() {
                            self.rollout_needed = true;
                            self.input.insert_str(self.input_cursor, &text);
                            self.input_cursor += text.len();
                        }
                    }
                    Some(Completion::ReplaceWholeLine {
                        new_line,
                        new_cursor,
                    }) => {
                        if new_cursor > new_line.len() {
                            panic!("Bug: Completor gave a new input cursor out of range for the string!");
                        }
                        if new_line != self.input
                            || new_cursor != self.input_cursor
                        {
                            self.rollout_needed = true;
                            self.input = new_line;
                            self.input_cursor = new_cursor;
                        }
                    }
                }
            }
        }
        #[cfg(not(feature = "completion"))]
        {
            // TODO: localized "completion is not supported" message?
            self.term.borrow_mut().bell()?;
        }
        Ok(())
    }
    pub fn rollin(&mut self) -> LifeOrDeath {
        if let Some(rem) = self.remembered_output.take() {
            self.rollout_needed = true;
            let mut term = self.term.borrow_mut();
            if self.input_allowed {
                term.hide_cursor()?;
            }
            if rem.cursor_left != 0 {
                term.carriage_return()?;
            }
            if rem.cursor_top != 0 {
                term.move_cursor_up(rem.cursor_top)?;
            }
            term.clear_forward_and_reset()?;
        }
        Ok(())
    }
    pub fn rollout(&mut self) -> LifeOrDeath {
        if !self.rollout_needed {
            return Ok(());
        }
        self.rollout_needed = false;
        let mut new_output = match self.status.as_ref() {
            None => Line::new(),
            Some(status) => {
                let mut line = status.clone();
                line.reset_and_break();
                line
            }
        };
        let cursor_pos;
        if let Some((line, _)) = self.notice.as_ref() {
            new_output.append_line(line);
            cursor_pos = None;
        } else {
            if let Some(line) = self.prompt.as_ref() {
                new_output.append_line(line);
            }
            cursor_pos = Some(self.input_cursor + new_output.len());
            new_output.add_text(&self.input);
        }
        self.term.borrow_mut().hide_cursor()?;
        self.output_line_changes(&new_output, cursor_pos, false, true)?;
        let mut term = self.term.borrow_mut();
        if self.notice.is_none() && self.input_allowed {
            term.show_cursor()?;
        }
        term.flush()?;
        Ok(())
    }
    fn cleanup(self) -> LifeOrDeath {
        RefCell::into_inner(self.term).cleanup()?;
        Ok(())
    }
}

/// This is the actual worker function we use when we're in "tty mode", that
/// is, we believe we have a terminal crossterm supports and NO PIPES.
fn tty_worker(
    req_tx: std_mpsc::Sender<Request>,
    rx: std_mpsc::Receiver<Request>,
    mut tx: tokio_mpsc::UnboundedSender<Response>,
    #[cfg(feature = "history")] history: Arc<RwLock<History>>,
) -> LifeOrDeath {
    let req_tx_clone = req_tx.clone();
    let (mut ded_tx, ded_rx) = std_mpsc::sync_channel(5);
    std::thread::Builder::new()
        .name("Liso heartbeat thread".to_owned())
        .spawn(move || {
            let mut deadlines = Vec::with_capacity(4);
            loop {
                if deadlines.is_empty() {
                    match ded_rx.recv() {
                        Ok(x) => deadlines.push(x),
                        Err(_) => break,
                    };
                } else {
                    let now = Instant::now();
                    if !deadlines.is_empty() && now >= deadlines[0] {
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
        })
        .unwrap();
    crossterm::terminal::enable_raw_mode()?;
    let term = new_term(&req_tx)?;
    let mut state = TtyState {
        status: None,
        prompt: None,
        notice: None,
        remembered_output: None,
        input_allowed: true,
        input: String::new(),
        input_cursor: 0,
        term: RefCell::new(term),
        rollout_needed: false,
        clipboard: String::new(),
        #[cfg(feature = "history")]
        history,
        #[cfg(feature = "history")]
        cur_history_index: None,
        #[cfg(feature = "history")]
        orphaned_new_input: None,
        #[cfg(feature = "history")]
        history_original_line: None,
        #[cfg(feature = "completion")]
        completor: None,
        #[cfg(feature = "completion")]
        consecutive_completion_presses: 0,
        #[cfg(feature = "completion")]
        own_output: Output { tx: req_tx },
    };
    let mut dying = false;
    'outer: while let Some(request) = if dying {
        rx.try_recv().ok()
    } else {
        rx.recv().ok()
    } {
        if let Request::Die = request {
            break;
        }
        state.handle(&mut tx, &mut ded_tx, request)?;
        loop {
            use std_mpsc::TryRecvError;
            match rx.try_recv() {
                Ok(Request::Die) => {
                    dying = true;
                    if cfg!(not(feature = "capture-stderr")) {
                        // if we're not capturing stderr, there's no reason not
                        // to break immediately
                        break;
                    }
                }
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

fn is_pipe_term(input: Option<&str>) -> bool {
    matches!(input, Some("dumb") | Some("pipe"))
}

pub(crate) fn worker(
    req_tx: std_mpsc::Sender<Request>,
    rx: std_mpsc::Receiver<Request>,
    tx: tokio_mpsc::UnboundedSender<Response>,
    #[cfg(feature = "history")] history: Arc<RwLock<History>>,
) -> LifeOrDeath {
    if !(std::io::stdout().is_tty() && std::io::stdin().is_tty())
        || is_pipe_term(
            std::env::var("TERM").as_ref().ok().map(String::as_str),
        )
    {
        pipe_worker(req_tx, rx, tx)
    } else {
        #[cfg(feature = "capture-stderr")]
        stderr_capture::attempt_stderr_capture(Output { tx: req_tx.clone() });
        #[cfg(feature = "history")]
        return tty_worker(req_tx, rx, tx, history);
        #[cfg(not(feature = "history"))]
        return tty_worker(req_tx, rx, tx);
    }
}
