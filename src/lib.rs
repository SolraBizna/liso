use std::{
    borrow::Cow,
    time::Duration,
    sync::mpsc as std_mpsc,
};

use bitflags::bitflags;
use crossterm::style::{
    Color as CtColor,
    Attribute as CtAttribute,
    Attributes as CtAttributes,
};
use crossterm::event::Event;
use tokio::sync::mpsc as tokio_mpsc;

mod worker;
mod term;
use term::*;

/// When handling input ourselves, this is the amount of time to wait after
/// receiving an escape before we're sure we don't have an escape sequence on
/// our hands.
///
/// This is fairly long to ensure that, even on a 300 baud modem, we would
/// *definitely* have received another character in the sequence before this
/// deadline elapses. (I say that it's fairly long, but curses waits an entire
/// **second**, which is much, much, much too long!)
///
/// If Crossterm input is being used, this is ignored.
const ESCAPE_DELAY: Duration = Duration::new(0, 1000000000 / 24);

/// We have to handle errors. There are two kinds we'll routinely face:
///
/// - Error writing to `Stdout`
/// - Error sending out a `Response`
///
/// The correct answer to both is to quietly, calmly, close down our thread. We
/// abuse the `?` operator to make this quick and easy. Since we don't actually
/// need any of the error information, we can condense it all down into this,
/// the "an error happened and we don't care what" type.
struct DummyError {}
type LifeOrDeath = std::result::Result<(),DummyError>;
impl From<std::io::Error> for DummyError {
    fn from(_: std::io::Error) -> DummyError { DummyError {} }
}
impl<T> From<tokio_mpsc::error::SendError<T>> for DummyError {
    fn from(_: tokio_mpsc::error::SendError<T>) -> DummyError { DummyError {} }
}
impl<T> From<std_mpsc::SendError<T>> for DummyError {
    fn from(_: std_mpsc::SendError<T>) -> DummyError { DummyError {} }
}
impl From<std_mpsc::RecvError> for DummyError {
    fn from(_: std_mpsc::RecvError) -> DummyError { DummyError {} }
}
impl From<std_mpsc::RecvTimeoutError> for DummyError {
    fn from(_: std_mpsc::RecvTimeoutError) -> DummyError { DummyError {} }
}

/// Colors we support outputting. For compatibility, we only support the 3-bit
/// ANSI colors. Use color sparingly.
///
/// Remember that some terminals don't support color at all, and that some
/// users will be using a different theme from you (white on black, black on
/// white, green on black, yellow on orange, solarized, something else with
/// scrambled/shuffled colors...).
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
#[repr(u8)]
pub enum Color {
    Black=0,
    Red=1,
    Green=2,
    Yellow=3,
    Blue=4,
    Cyan=5,
    Magenta=6,
    White=7,
}

impl Color {
    // Convert to a Crossterm color
    fn to_crossterm(self) -> CtColor {
        match self {
            Color::Black => CtColor::Black,
            Color::Red => CtColor::DarkRed,
            Color::Green => CtColor::DarkGreen,
            Color::Yellow => CtColor::DarkYellow,
            Color::Blue => CtColor::DarkBlue,
            Color::Cyan => CtColor::DarkCyan,
            Color::Magenta => CtColor::DarkMagenta,
            Color::White => CtColor::Grey,
        }
    }
    // Convert to an Atari ST 16-color color index (bright)
    fn to_atari16_bright(self) -> u8 {
        match self {
            Color::Black => 8,
            Color::Red => 1,
            Color::Green => 2,
            Color::Yellow => 13,
            Color::Blue => 4,
            Color::Cyan => 9,
            Color::Magenta => 12,
            Color::White => 0,
        }
    }
    // Convert to an Atari ST 16-color color index (dim)
    fn to_atari16_dim(self) -> u8 {
        match self {
            Color::Black => 15,
            Color::Red => 3,
            Color::Green => 5,
            Color::Yellow => 11,
            Color::Blue => 6,
            Color::Cyan => 10,
            Color::Magenta => 14,
            Color::White => 7,
        }
    }
    // Convert to an Atari ST 4-color color index
    fn to_atari4(self) -> u8 {
        match self {
            Color::Black => 15,
            Color::Red => 1,
            Color::Green => 2,
            Color::Yellow => 2,
            Color::Blue => 3,
            Color::Cyan => 2,
            Color::Magenta => 1,
            Color::White => 0,
        }
    }
}

bitflags! {
    /// Styles we support outputting.
    ///
    /// Some terminals don't support any of this, and some don't support all of
    /// it. On any standards-compliant terminal, unsupported features will be
    /// ignored. Even on standards-compliant terminals, these are very open to
    /// interpretation.
    #[derive(Default)]
    pub struct Style: u32 {
        const PLAIN = 0;
        /// Prints in a bolder font and/or a brighter color.
        const BOLD = 1 << 0;
        /// Prints in a thinner font and/or a dimmer color.
        const DIM = 1 << 1;
        /// Prints with a line under the baseline.
        const UNDERLINE = 1 << 2;
        /// Prints with the foreground and background colors reversed. (Some
        /// terminals that don't support color do support this.)
        ///
        /// Liso toggles this whenever it's outputting a control sequence.
        const INVERSE = 1 << 3;
    }
}

impl Style {
    fn to_crossterm(&self) -> CtAttributes {
        let mut ret = CtAttributes::default();
        if self.contains(Style::BOLD) { ret.set(CtAttribute::Bold) }
        if self.contains(Style::DIM) { ret.set(CtAttribute::Dim) }
        if self.contains(Style::UNDERLINE) { ret.set(CtAttribute::Underlined) }
        if self.contains(Style::INVERSE) { ret.set(CtAttribute::Reverse) }
        ret
    }
}

/// Sends output to the terminal. You can have more than one of these, shared
/// freely among threads and tasks. Give one to every thread that needs to
/// produce output.
#[derive(Clone)]
pub struct Sender {
    tx: std_mpsc::Sender<Request>,
}

/// Receives input from the terminal. Only one thread can have this privilege
/// at a time. Also acts as a [`Sender`](struct.Sender.html) for sending output
/// to the terminal. Use `clone_sender` to branch additional `Sender`s off for
/// use threads other than the one receiving input.
pub struct IO {
    sender: Sender,
    rx: tokio_mpsc::UnboundedReceiver<Response>,
}

/// An individual styled span within a line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LineElement {
    /// The style in effect.
    style: Style,
    /// The foreground color (if any).
    fg: Option<Color>,
    /// The background color (if any).
    bg: Option<Color>,
    /// The start (inclusive) and end (exclusive) range of text within the
    /// parent `Line` to which these attributes apply.
    start: usize, end: usize,
}

/// This is a line of text, with optional styling information, ready for
/// display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    text: String,
    elements: Vec<LineElement>,
}

impl Line {
    /// Creates a new, empty line.
    pub fn new() -> Line {
        Line { text: String::new(), elements: Vec::new() }
    }
    /// Creates a new line, containing the given, unstyled, text. Creates a new
    /// copy iff the passed `Cow` is borrowed or contains control characters.
    pub fn from_cow(i: Cow<str>) -> Line {
        let mut ret = Line::new();
        ret.add_text(i);
        ret
    }
    /// Creates a new line, containing the given, unstyled, text. Always copies
    /// the passed string.
    pub fn from_str(i: &str) -> Line {
        Line::from_cow(Cow::Borrowed(i))
    }
    /// Creates a new line, containing the given, unstyled, text. Creates a new
    /// copy iff the passed `String` contains control characters.
    pub fn from_string(i: String) -> Line {
        Line::from_cow(Cow::Owned(i))
    }
    /// Creates a new line, containing the given, ANSI-styled, text. Creates
    /// a new copy iff the passed text contains control characters or ANSI
    /// escape sequences.
    pub fn from_ansi<'a, T>(&mut self, i: T) -> Line
    where T: Into<Cow<'a, str>> {
        let mut ret = Line::new();
        ret.add_ansi(i);
        ret
    }
    fn append_text(&mut self, i: Cow<str>) {
        if i.len() == 0 { return }
        if self.text.len() == 0 {
            // The line didn't have any text or elements yet.
            match self.elements.last_mut() {
                None => {
                    self.elements.push(LineElement {
                        style: Style::PLAIN, fg: None, bg: None,
                        start: 0, end: i.len()
                    });
                },
                Some(x) => {
                    assert_eq!(x.start, 0);
                    assert_eq!(x.end, 0);
                    x.end = i.len();
                },
            }
            self.text = i.into_owned();
        }
        else {
            // The line did have some text.
            let start = self.text.len();
            let end = start + i.len();
            self.text += &i[..];
            let endut = self.elements.last_mut().unwrap();
            assert_eq!(endut.end, start);
            endut.end = end;
        }
    }
    /// Adds additional text to the `Line` using the current styling.
    pub fn add_text<'a, T>(&mut self, i: T) -> &mut Line
    where T: Into<Cow<'a, str>> {
        let i: Cow<str> = i.into();
        if i.len() == 0 { return self }
        // we regard as a control character anything in the C0 and C1 control
        // character blocks, as well as the U+2028 LINE SEPARATOR and
        // U+2029 PARAGRAPH SEPARATOR characters. Except newline!
        let mut control_iterator = i.match_indices(|x: char|
                                                   (x.is_control()
                                                    && x != '\n')
                                                   || x == '\u{2028}'
                                                   || x == '\u{2029}');
        let first_control_pos = control_iterator.next();
        match first_control_pos {
            None => {
                // No control characters to expand. Put it in directly.
                self.append_text(i);
            },
            Some(mut pos) => {
                let mut plain_start = 0;
                loop {
                    if pos.0 != plain_start {
                        self.append_text(Cow::Borrowed(&i[plain_start..pos.0]));
                    }
                    let control_char = pos.1.chars().next().unwrap();
                    self.toggle_style(Style::INVERSE);
                    let control_char = control_char as u32;
                    let addendum = if control_char < 32 {
                        format!("^{}", (b'@'+(control_char as u8))
                                        as char)
                    }
                    else {
                        format!("U+{:04X}", control_char)
                    };
                    self.append_text(Cow::Owned(addendum));
                    self.toggle_style(Style::INVERSE);
                    plain_start = pos.0 + pos.1.len();
                    match control_iterator.next() {
                        None => break,
                        Some(nu) => pos = nu,
                    }
                }
                if plain_start != i.len() {
                    self.append_text(Cow::Borrowed(&i[plain_start..]));
                }
            },
        }
        self
    }
    /// Adds additional text to the `Line`, starting with the current styling,
    /// and applying any ANSI control sequences we can understand.
    ///
    /// Strips any control sequences other than Select Graphics Rendition,
    /// as well as any Graphics Rendition sequences we don't know.
    pub fn add_ansi<'a, T>(&mut self, _i: T) -> &mut Line
    where T: Into<Cow<'a, str>> {
        todo!()
    }
    /// Returns the Style in effect at the end of the line, as it exists now.
    pub fn get_style(&self) -> Style {
        match self.elements.last() {
            None => Style::PLAIN,
            Some(x) => x.style,
        }
    }
    /// Change the active Style to exactly those given.
    pub fn set_style(&mut self, nu: Style) -> &mut Line {
        let (fg, bg) = match self.elements.last_mut() {
            // case 1: no elements yet, make one.
            None => {
                // (fall through)
                (None, None)
            },
            Some(x) => {
                // case 2: no change to attributes
                if x.style == nu { return self }
                // case 3: last element doesn't have text yet.
                else if x.start == x.end { x.style = nu; return self }
                (x.fg, x.bg)
            },
        };
        // (case 1 fall through, or...)
        // case 4: an element with text is here.
        self.elements.push(LineElement {
            style: nu, fg, bg,
            start: self.text.len(), end: self.text.len(),
        });
        self
    }
    /// Toggle the given Styles. For every style passed in, if it is set, it
    /// will be unset, and vice versa.
    pub fn toggle_style(&mut self, nu: Style) -> &mut Line {
        let old = self.get_style();
        self.set_style(old ^ nu)
    }
    /// Activate the given Styles.
    pub fn activate_style(&mut self, nu: Style) -> &mut Line {
        let old = self.get_style();
        self.set_style(old | nu)
    }
    /// Deactivate the given Styles.
    pub fn deactivate_style(&mut self, nu: Style) -> &mut Line {
        let old = self.get_style();
        self.set_style(old - nu)
    }
    /// Deactivate all Styles. Same as calling `set_style(Style::PLAIN)`.
    pub fn clear_style(&mut self) -> &mut Line {
        self.set_style(Style::PLAIN)
    }
    /// Gets the current colors, foreground and background.
    pub fn get_colors(&self) -> (Option<Color>, Option<Color>) {
        match self.elements.last() {
            None => (None, None),
            Some(x) => (x.fg, x.bg),
        }
    }
    /// Sets the foreground color.
    pub fn set_fg_color(&mut self, nu: Option<Color>) -> &mut Line {
        let (fg, bg) = self.get_colors();
        if nu != fg { self.set_colors(nu, bg); }
        self
    }
    /// Sets the background color.
    pub fn set_bg_color(&mut self, nu: Option<Color>) -> &mut Line {
        let (fg, bg) = self.get_colors();
        if nu != bg { self.set_colors(fg, nu); }
        self
    }
    /// Sets the foreground and background color.
    pub fn set_colors(&mut self, fg: Option<Color>, bg: Option<Color>) -> &mut Line {
        let prev_style = match self.elements.last_mut() {
            // case 1: no elements yet, make one.
            None => Style::PLAIN,
            Some(x) => {
                // case 2: no change to style
                if x.fg == fg && x.bg == bg { return self }
                // case 3: last element doesn't have text yet.
                else if x.start == x.end { x.fg = fg; x.bg = bg; return self }
                x.style
            },
        };
        // (case 1 fall through, or...)
        // case 3: an element with text is here.
        self.elements.push(LineElement {
            style: prev_style, fg, bg,
            start: self.text.len(), end: self.text.len(),
        });
        self
    }
    /// Returns true if this line contains no text.
    pub fn is_empty(&self) -> bool { self.text.is_empty() }
    /// Returns the number of **BYTES** of text this line contains.
    pub fn len(&self) -> usize { self.text.len() }
    /// Iterate over chars of the line, including style information, one char
    /// at a time.
    ///
    /// Yields: `(byte_index, character, style, fgcolor, bgcolor)`
    pub fn chars(&self) -> LineCharIterator<'_> {
        LineCharIterator::new(self)
    }
    /// Add a linebreak and clear style and color.
    pub fn clear_and_break(&mut self) {
        self.add_text("\n");
        self.set_style(Style::empty());
        self.set_colors(None, None);
    }
    /// Append another Line to ourselves, including style information. You may
    /// want to `clear_and_break first.
    pub fn append_line(&mut self, other: &Line) {
        for element in other.elements.iter() {
            self.set_style(element.style);
            self.set_colors(element.fg, element.bg);
            self.add_text(&other.text[element.start .. element.end]);
        }
    }
    /// Insert linebreaks to wrap to the given number of columns. Only
    /// available with the "wrap" feature, which is enabled by default.
    #[cfg(feature="wrap")]
    pub fn wrap_to_width(&mut self, width: usize) {
        assert!(width > 0);
        let wrap_vec = textwrap::wrap(&self.text, width);
        let mut edit_vec = Vec::with_capacity(wrap_vec.len());
        let mut cur_end = 0;
        for el in wrap_vec.into_iter() {
            // We're pretty sure we didn't use any features that would require
            // an owned Cow. In fact, if we're wrong, the whole feature won't
            // work.
            let slice = match el {
                Cow::Borrowed(x) => x,
                Cow::Owned(_)
                => panic!("We needed textwrap to do borrows only!"),
            };
            let (start, end) = convert_subset_slice_to_range(&self.text,slice);
            debug_assert!(start <= end);
            if start == end { continue }
            assert!(start >= cur_end);
            if start != 0 {
                edit_vec.push(cur_end..start);
            }
            cur_end = end;
        }
        for range in edit_vec.into_iter().rev() {
            self.erase_and_insert_newline(range);
        }
    }
    // Internal use only.
    #[cfg(feature="wrap")]
    fn erase_and_insert_newline(&mut self, range: std::ops::Range<usize>) {
        let delta_bytes = range.end as isize - range.start as isize - 1;
        self.text.replace_range(range.clone(), "\n");
        let mut elements_len = self.elements.len();
        let mut i = self.elements.len();
        loop {
            if i == 0 { break }
            i -= 1;
            let element = &mut self.elements[i];
            if element.end > range.end {
                element.end = ((element.end as isize) + delta_bytes) as usize;
            }
            else if element.end > range.start {
                element.end = range.start;
            }
            if element.start > range.end {
                element.start = ((element.start as isize) + delta_bytes) as usize;
            }
            else if element.start > range.start {
                element.start = range.start;
            }
            if element.end <= element.start {
                if i == elements_len-1 {
                    // preserve the last element, even if empty
                    element.end = element.start;
                }
                else {
                    drop(element);
                    self.elements.remove(i);
                    elements_len -= 1;
                    continue;
                }
            }
            if element.start >= range.start {
                break; // all subsequent elements will be before the edit
            }
        }
    }
}

impl Into<Line> for String {
    fn into(self) -> Line { Line::from_string(self) }
}

impl Into<Line> for &str {
    fn into(self) -> Line { Line::from_str(self) }
}

impl Into<Line> for Cow<'_, str> {
    fn into(self) -> Line { Line::from_cow(self) }
}

/// Something sent *to* the Liso thread.
enum Request {
    /// Sent by `println`
    Output(Line),
    /// Sent by `wrapln`
    #[cfg(feature="wrap")]
    OutputWrapped(Line),
    /// Sent by `status`
    Status(Option<Line>),
    /// Sent by `notice`
    Notice(Line, Duration),
    /// Sent by `prompt`
    Prompt {
        line: Option<Line>,
        input_allowed: bool,
        clear_input: bool,
    },
    /// Sent by the input task, when some input is inputted
    Bell,
    /// Sent when we're cleaning up
    Die,
    /// Sent whenever some raw input is received. This is an implementation
    /// detail of the specific worker used; for the pipe worker, this is an
    /// entire line, and for the tty worker, this is a block of raw input.
    ///
    /// Raw input is printable characters and simple control characters. Any
    /// possible, meaningful escape sequences must already have been parsed
    /// out. (The pipe worker doesn't interpret escape sequences and therefore
    /// does no such processing.)
    RawInput(String),
    /// Another implementation detail, used to implement notices.
    Heartbeat,
    /// Another implementation detail. If the crossterm event system is being
    /// used, this is an event received. This can be the case even if the
    /// crossterm *input* system isn't being used.
    CrosstermEvent(crossterm::event::Event),
}

/// Input received from the user, or a special condition.
/// 
/// If a control character isn't listed here (e.g. control-C, control-D)
/// then you can't assume you can receive it. It might have some meaning
/// to the line editor. (e.g. control-A -> go to beginning of line,
/// control-E -> go to end of line, control-W -> delete word...)
#[derive(Debug,PartialEq,Eq,PartialOrd,Ord)]
pub enum Response {
    /// Sent when the terminal or the IO thread have died.
    Dead,
    /// Sent when the user finishes entering a line of input.
    Input(String),
    /// Sent when the user types control-C, which normally means they want your
    /// program to quit.
    Quit,
    /// Sent when the user types control-Z, which normally means they want your
    /// program to suspend itself.
    Suspend,
    /// Sent when the user types control-D on an empty line, which normally
    /// means that they are done providing input.
    Finish,
    /// Sent when the user types control-T, which on some BSDs is a standard
    /// way to request that a program give a status report or other progress
    /// information.
    Info,
    /// Sent when the user types control-backslash, or when a break condition
    /// is detected. The meaning of this is application-specific.
    Break,
    /// Sent when the user presses Escape.
    Escape,
    /// Sent when the user presses control-X.
    Swap,
    /// Sent when the user presses an unknown control character with the given
    /// value (which will be between 0 and 31 inclusive).
    Unknown(u8),
}

impl Response {
    /// Returns the control code that triggered this response, e.g. 10 for
    /// `Input`, 3 for `Quit`, ... Useful if you want to produce a generic
    /// "unknown key ^X" kind of message for all the various optional keys you
    /// might not want to handle:
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # use liso::Response;
    /// # let response = Response::Quit;
    /// # let io = liso::IO::new();
    /// match response {
    ///     Response::Input(_) => { /* handle input somehow */ },
    ///     Response::Quit => return,
    ///     other => {
    ///         io.notice(format!("unknown key {}",
    ///                           other.as_unknown() as char),
    ///                   Duration::from_secs(1));
    ///     }
    /// }
    /// ```
    ///
    /// (note that Liso converts control characters to reverse-video ^X forms
    /// on display)
    pub fn as_unknown(&self) -> u8 {
        match self {
            &Response::Input(_) => 10,
            &Response::Quit => 3,
            &Response::Suspend => 26,
            &Response::Finish => 4,
            &Response::Info => 20,
            &Response::Dead | &Response::Break => 28,
            &Response::Escape => 27,
            &Response::Swap => 24,
            &Response::Unknown(x) => x,
        }
    }
}

impl Sender {
    /// Prints a (possibly styled) line of regular output to the screen.
    pub fn println<T>(&self, line: T)
    where T: Into<Line> {
        let _ = self.tx.send(Request::Output(line.into()));
    }
    /// Prints a (possibly styled) line of regular output to the screen,
    /// wrapping it to the width of the terminal. Only available with the
    /// "wrap" feature, which is enabled by default.
    pub fn wrapln<T>(&self, line: T)
    where T: Into<Line> {
        let _ = self.tx.send(Request::OutputWrapped(line.into()));
    }
    /// Sets the status line to the given (possibly styled) text.
    pub fn status<T>(&self, line: Option<T>)
    where T: Into<Line> {
        let _ = self.tx.send(Request::Status(line.map(T::into)));
    }
    /// Displays a (possibly styled) notice that temporarily replaces the
    /// prompt. Will disappear if the user types a key, or after the given
    /// amount of time passes.
    ///
    /// Replaces any previous notice.
    pub fn notice<T>(&self, line: T, max_duration: Duration)
    where T: Into<Line> {
        let _ = self.tx.send(Request::Notice(line.into(), max_duration));
    }
    /// Sets the prompt to the given (possibly styled) text.
    /// 
    /// `input_allowed`: True if the user should be allowed to write input.
    /// `clear_input`: True if any existing partial input should be cleared.
    /// 
    /// Note: If the prompt is styled, whatever style is active at the end of
    /// the prompt will be active for the user's input.
    pub fn prompt<T>(&self, line: T,
                     input_allowed: bool, clear_input: bool)
    where T: Into<Line> {
        let _ = self.tx.send(Request::Prompt {
            line: Some(line.into()), input_allowed, clear_input
        });
    }
    /// Removes the prompt.
    /// 
    /// `input_allowed`: True if the user should (still) be allowed to write
    ///   input.
    /// `clear_input`: True if any existing partial input should be cleared.
    pub fn remove_prompt(&self, input_allowed: bool, clear_input: bool) {
        let _ = self.tx.send(Request::Prompt {
            line: None, input_allowed, clear_input
        });
    }
    /// Get the user's attention with an audible or visible bell.
    pub fn bell(&self) {
        let _ = self.tx.send(Request::Bell);
    }
}

impl Drop for IO {
    fn drop(&mut self) {
        self.actually_blocking_die()
    }
}

impl core::ops::Deref for IO {
    type Target = Sender;
    fn deref(&self) -> &Sender { &self.sender }
}

impl IO {
    pub fn new() -> IO {
        let (request_tx, request_rx) = std_mpsc::channel();
        let (response_tx, response_rx) = tokio_mpsc::unbounded_channel();
        let request_tx_clone = request_tx.clone();
        std::thread::Builder::new().name("Liso output thread".to_owned())
            .spawn(move || {
                let _ =
                    worker::worker(request_tx_clone, request_rx, response_tx);
            })
            .unwrap();
        IO {
            sender: Sender { tx: request_tx },
            rx: response_rx,
        }
    }
    /// An `IO` instance contains both a `Sender` (to produce output) and a
    /// receiver (to receive input). Multiple `Sender`s may coexist in the same
    /// program; produce additional `Sender`s as needed with this function.
    pub fn clone_sender(&self) -> Sender {
        self.sender.clone()
    }
    /// Erase the prompt/status lines, put the terminal in a sensible mode,
    /// and otherwise clean up everything we've done to the terminal. You
    /// may need to make sure this gets called when your program terminates.
    ///
    /// If you made copies of this `Sender`, they will be "dead"; calling their
    /// methods won't panic, but it won't do anything else either.
    pub async fn die(mut self) {
        if self.sender.tx.send(Request::Die).is_err() {
            // already dead!
            return
        }
        loop {
            match self.read().await {
                Response::Dead => break,
                _ => (),
            }
        }
    }
    fn actually_blocking_die(&mut self) {
        if self.sender.tx.send(Request::Die).is_err() {
            // already dead!
            return
        }
        loop {
            match self.blocking_read() {
                Response::Dead => break,
                _ => (),
            }
        }
    }
    pub fn blocking_die(mut self) {
        self.actually_blocking_die()
    }
    pub async fn read(&mut self) -> Response {
        match self.rx.recv().await {
            None => Response::Dead,
            Some(x) => x,
        }
    }
    pub fn blocking_read(&mut self) -> Response {
        match self.rx.blocking_recv() {
            None => Response::Dead,
            Some(x) => x,
        }
    }
    pub fn try_read(&mut self) -> Option<Response> {
        use tokio::sync::mpsc::error::TryRecvError;
        match self.rx.try_recv() {
            Ok(x) => Some(x),
            Err(TryRecvError::Disconnected) => Some(Response::Dead),
            Err(TryRecvError::Empty) => None,
        }
    }
}

pub struct LineCharIterator<'a> {
    line: &'a Line,
    cur_element: usize,
    indices: std::str::CharIndices<'a>,
}

/// A char from a `Line`, with associated style and index information.
#[derive(Clone,Copy,Debug)]
pub struct LineChar {
    /// Byte index within the `Line` of this char.
    pub index: usize,
    /// The actual char.
    pub ch: char,
    /// Style (bold, inverse, etc.)
    pub style: Style,
    /// Foreground color
    pub fg: Option<Color>,
    /// Background color
    pub bg: Option<Color>,
}

impl PartialEq for LineChar {
    fn eq(&self, other: &LineChar) -> bool {
        self.ch == other.ch && self.style == other.style && self.fg == other.fg
            && self.bg == other.bg
    }
}

impl LineChar {
    /// Returns true if it is definitely impossible to distinguish spaces
    /// printed in the style of both LineChars, false if it might be possible
    /// to distinguish them. Used to optimize endfill when overwriting one line
    /// with another.
    ///
    /// In cases whether the answer depends on the specific terminal, returns
    /// false. One example is going from inverse video with a foreground color
    /// to non-inverse video with the corresponding background color. (Some
    /// terminals will display the same color differently depending on whether
    /// it's foreground or background, and some of those terminals implement
    /// inverse by simply swapping foreground and background, therefore we
    /// can't count on them looking the same just because the color index is
    /// the same.)
    pub fn endfills_same_as(&self, other: &LineChar) -> bool {
        let a_underline = self.style.contains(Style::UNDERLINE);
        let b_underline = other.style.contains(Style::UNDERLINE);
        if a_underline != b_underline { return false }
        debug_assert_eq!(a_underline, b_underline);
        let a_inverse = self.style.contains(Style::INVERSE);
        let b_inverse = other.style.contains(Style::INVERSE);
        if a_inverse != b_inverse { false }
        else if a_inverse {
            debug_assert!(b_inverse);
            if a_underline && self.bg != other.bg { return false }
            self.fg == other.fg
        }
        else {
            debug_assert!(!a_inverse);
            debug_assert!(!b_inverse);
            if a_underline && self.fg != other.fg { return false }
            self.bg == other.bg
        }
    }
}

impl<'a> LineCharIterator<'a> {
    fn new(line: &'a Line) -> LineCharIterator<'a> {
        LineCharIterator {
            line,
            cur_element: 0,
            indices: line.text.char_indices(),
        }
    }
}

impl Iterator for LineCharIterator<'_> {
    type Item = LineChar;
    fn next(&mut self) -> Option<LineChar> {
        let (index, ch) = match self.indices.next() {
            Some(x) => x,
            None => return None,
        };
        while self.cur_element < self.line.elements.len()
        && self.line.elements[self.cur_element].end <= index {
            self.cur_element += 1;
        }
        // We should never end up with text in the text string that is not
        // covered by an element.
        debug_assert!(self.cur_element < self.line.elements.len());
        let element = &self.line.elements[self.cur_element];
        Some(LineChar {
            index, ch,
            style: element.style,
            fg: element.fg,
            bg: element.bg,
        })
    }
}

#[cfg(feature="wrap")]
fn convert_subset_slice_to_range(outer: &str, inner: &str) -> (usize, usize) {
    let outer_start = outer.as_ptr() as usize;
    let outer_end = outer_start.checked_add(outer.len()).unwrap();
    let inner_start = inner.as_ptr() as usize;
    let inner_end = inner_start.checked_add(inner.len()).unwrap();
    assert!(inner_start >= outer_start);
    assert!(inner_end <= outer_end);
    (inner_start - outer_start, inner_end - outer_start)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_char_splatting() {
        let mut line = Line::new();
        line.add_text("Escape: \u{001B} Some C1 code: \u{008C} \
                       Paragraph separator: \u{2029}");
        assert_eq!(line.text,
                   "Escape: ^[ Some C1 code: U+008C \
                    Paragraph separator: U+2029");
        assert_eq!(line.elements.len(), 7);
        assert_eq!(line.elements[0].style, Style::PLAIN);
        assert_eq!(line.elements[1].style, Style::INVERSE);
        assert_eq!(line.elements[2].style, Style::PLAIN);
    }
    #[test] #[cfg(feature="wrap")]
    fn line_wrap_splat() {
        for n in 1 .. 200 {
            let mut line = Line::new();
            line.add_text("This is ");
            line.set_style(Style::BOLD);
            line.add_text("a test");
            line.set_style(Style::empty());
            line.add_text(" of line wrapping?");
            line.wrap_to_width(n);
        }
    }
}
