//! Liso (LEE-soh) is an acronym for Line Input with Simultaneous Output. It is
//! a library for a particular kind of text-based Rust application; one where
//! the user is expected to give command input at a prompt, but output can
//! occur at any time. It provides simple line editing, and prevents input from
//! clashing with output. It can be used asynchronously (with `tokio`) or
//! synchronously (without).
//!
//! # Usage
//! 
//! Create an [`IO`](struct.IO.html) object with `IO::new()`. Liso will
//! automatically configure itself based on how your program is being used.
//! 
//! Your `IO` instance can be used to send output or receive input. Call
//! `clone_sender` to create a `Sender` instance, which can only be used to
//! send output. You can call `clone_sender` as many times as you like, as well
//! as cloning the `Sender`s directly. An unlimited number of threads/tasks can
//! send output through Liso, but only one thread/task can receive user input:
//! whichever one currently holds the `IO` instance.
//! 
//! Liso can work with `String`s and `&str`s directly. If you want to add style
//! or color, create a [`Line`](struct.Line.html), either manually or using
//! the convenient [`liso!` macro](macro.liso.html). Send output to the
//! user by calling [`println()`](struct.Sender.html#method.println) or
//! [`wrapln()`](struct.Sender.html#method.wrapln), whichever you prefer. Any
//! styling and color information is reset after the line is output, so you
//! don't have to worry about dangling attributes.
//! 
//! Liso supports a prompt line, which is presented ahead of the user input.
//! Use [`prompt()`](struct.Sender.html#method.prompt) to set it. Styling and
//! color information is *not* reset between the prompt and the current input
//! text, so you can style/color the input text by having the desired
//! styles/colors active at the end of the prompt line.
//! 
//! Liso supports an optional status line, which "hangs out" above the input
//! text. Use [`status()`](struct.Sender.html#method.status) to set it. Printed
//! text appears above the status line, the prompt and any in-progress input
//! appears below it. Use this to present contextual or frequently-changing
//! information.
//! 
//! Liso supports "notices", temporary messages that appear in place of the
//! prompt and input for a limited time. Use
//! [`notice()`](struct.Sender.html#method.notice) to display one. The notice
//! will disappear when the allotted time elapses, when the user presses any
//! key, or when another notice is displayed, whichever happens first. You
//! should only use this in direct response to user input; in fact, the only
//! legitimate use may be to complain about an unknown control character. (See
//! [`Response::as_unknown`](enum.Response.html#method.as_unknown) for an
//! example of this use.)
//! 
//! # Pipe mode
//! 
//! If *either* stdin or stdout is not a tty, *or* the `TERM` environment
//! variable is set to either `dumb` or `pipe`, Liso enters "pipe mode". In
//! this mode, status lines, notices, and prompts are not outputted, style
//! information is discarded, and every line of input is passed directly to
//! your program without any processing of control characters or escape
//! sequences. This means that a program using Liso will behave nicely when
//! used in a pipeline, or with a relatively unsophisticated terminal.
//! 
//! `TERM=dumb` is respected out of backwards compatibility with old UNIXes and
//! real terminals that identify this way. `TERM=pipe` is present as an
//! alternative for those who would rather not perpetuate an ableist slur, but
//! is not compatible with other UNIX utilities and conventions. On UNIX. you
//! can activate "pipe mode" without running afoul of any of this by piping the
//! output of the Liso-enabled program to `cat`, as in `my_liso_program | cat`.

use std::{
    borrow::Cow,
    time::{Duration, Instant},
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
/// ANSI colors.
/// 
/// Here's a short list of reasons not to use color as the only source of
/// certain information:
///
/// - Some terminals don't support color at all.
/// - Some terminals support color, but not all the ANSI colors. (e.g. the
///   Atari ST's VT52 emulator in medium-res mode, which supports white, black,
///   red, and green.)
/// - Some users will be using unexpected themes. White on black, black on
///   white, green on black, yellow on orange, and "Solarized" are all common.
/// - Many users have some form of colorblindness. The most common form,
///   affecting as much as 8% of the population, would make `Red`, `Yellow`,
///   and `Green` hard to distinguish from one another. Every other imaginable
///   variation also exists.
/// 
/// And some guidelines to adhere to:
/// 
/// - Never specify a foreground color of `White` or `Black` without also
///   specifying a background color, or vice versa.
/// - Instead of setting white-on-black or black-on-white, consider using
///   [inverse video](struct.Style.html#associatedconstant.INVERSE) to achieve
///   your goal instead.
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
        /// No styling at all. (A nice alias for `Style::empty()`.)
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
        /// Liso toggles this whenever it's outputting a control sequence:
        ///
        /// ```rust
        /// # use liso::liso;
        /// assert_eq!(liso!("Type \x03 to quit."),
        ///            liso!("Type ", ^inverse, "^C", ^inverse, " to quit."));
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

/// Receives input from, and sends output to, the terminal. You can *send
/// output* from any number of threads
/// (see [`IO::clone_sender`](struct.IO.html#method.clone_sender)), but only
/// one thread at a time may have ownership of the overlying `IO` type and
/// therefore the ability to *receive input*.
pub struct IO {
    sender: Sender,
    rx: tokio_mpsc::UnboundedReceiver<Response>,
    death_count: u32,
}

/// Number of times that we will report `Response::Dead` before we decide that
/// our caller isn't handling it correctly, and panic.
const MAX_DEATH_COUNT: u32 = 9;

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
/// display. The [`liso!` macro](macro.liso.html) is extremely convenient for
/// building these.
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
    /// Returns all the text in the line, without any styling information.
    pub fn as_str(&self) -> &str {
        &self.text
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
        // U+2029 PARAGRAPH SEPARATOR characters. Except newliso!
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
    /// Reset ALL style and color information to default. Equivalent to calling
    /// `set_style(Style::PLAIN)` followed by `set_colors(None, None)`.
    pub fn reset_all(&mut self) -> &mut Line {
        self.set_style(Style::PLAIN).set_colors(None, None)
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
    /// Add a linebreak and then clear style and color.
    pub fn reset_and_break(&mut self) {
        self.add_text("\n");
        self.set_style(Style::empty());
        self.set_colors(None, None);
    }
    /// Append another Line to ourselves, including style information. You may
    /// want to `reset_and_break` first.
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
    #[doc(hidden)]
    RawInput(String),
    /// Another implementation detail, used to implement notices.
    #[doc(hidden)]
    Heartbeat,
    /// Another implementation detail. If the crossterm event system is being
    /// used, this is an event received. This can be the case even if the
    /// crossterm *input* system isn't being used.
    #[doc(hidden)]
    CrosstermEvent(crossterm::event::Event),
}

/// Input received from the user, or a special condition.
/// 
/// If a control character isn't listed here (e.g. control-C, control-D)
/// then you can't assume you can receive it. It might have some meaning
/// to the line editor. (e.g. control-A -> go to beginning of line,
/// control-E -> go to end of line, control-W -> delete word...)
#[derive(Debug,PartialEq,Eq,PartialOrd,Ord)]
#[non_exhaustive]
pub enum Response {
    /// Sent when the terminal or the IO thread have died. Once you receive
    /// this once, you will never receive any other `Response` from Liso again.
    /// Your program should exit soon after, or at the very least should close
    /// down that `IO` instance.
    /// 
    /// If your program receives `Response::Dead` on the same `IO` instance
    /// too many times, Liso will panic. This is to prevent poorly-written
    /// programs from failing to exit after a hangup condition or bug in
    /// Liso cut off user input.
    Dead,
    /// Sent when the user finishes entering a line of input. This is the
    /// entire line.
    Input(String),
    /// Sent when the user types control-C, which normally means they want your
    /// program to quit.
    Quit,
    /// Sent when the user types control-Z, which normally means they want your
    /// program to suspend itself.
    Suspend,
    /// Sent when the user types control-D on an empty line, which normally
    /// means that they are done providing input (possibly temporarily).
    Finish,
    /// Sent when the user types control-T, which on some BSDs is a standard
    /// way to request that a program give a status report or other progress
    /// information.
    Info,
    /// Sent when the user types control-backslash, or when a break condition
    /// is detected. The meaning of this is application-specific. If you're
    /// running on a real, physical terminal line, this usually indicates an
    /// excessively noisy line, or a disconnect ("break") in the line.
    Break,
    /// Sent when the user presses Escape.
    Escape,
    /// Sent when the user presses control-X.
    Swap,
    /// Sent when the user presses an unknown control character with the given
    /// value (which will be between 0 and 31 inclusive).
    /// 
    /// Don't use particular values of `Unknown` for any specific purpose.
    /// Later versions of Liso may add additional `Response` variants for new
    /// control keys, or handle more control keys itself, replacing the
    /// `Unknown(...)` values those keys used to send. See
    /// [`as_unknown`](#method.as_unknown) for an example of how this variant
    /// should be used (i.e. not directly).
    Unknown(u8),
}

impl Response {
    /// Returns the control code that triggered this response, e.g. 10 for
    /// `Input`, 3 for `Quit`, ... Use this to produce a generic "unknown key
    /// key ^X" kind of message for any `Response` variants you don't handle,
    /// perhaps with code like:
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # use liso::Response;
    /// # let response = Response::Quit;
    /// # let io = liso::IO::new();
    /// match response {
    ///     Response::Input(_) => { /* handle input somehow */ },
    ///     Response::Quit | Response::Dead => return,
    ///     other => {
    ///         io.notice(format!("unknown key {}",
    ///                           other.as_unknown() as char),
    ///                   Duration::from_secs(1));
    ///     }
    /// }
    /// ```
    ///
    /// (Liso converts control characters to reverse-video ^X forms on display,
    /// so this will display like "unknown key ^X" with the "^X" hilighted.)
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
            death_count: 0,
        }
    }
    /// An `IO` instance contains both a `Sender` (to produce output) and a
    /// receiver (to receive input). Multiple `Sender`s may coexist in the same
    /// program; produce additional `Sender`s as needed with this function.
    pub fn clone_sender(&self) -> Sender {
        self.sender.clone()
    }
    /// Erase the prompt/status lines, put the terminal in a sensible mode,
    /// and otherwise clean up everything we've done to the terminal. This will
    /// happen automatically when this `IO` instance is dropped; you only need
    /// this method if you want to shut Liso down asynchronously for some
    /// reason.
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
    /// Erase the prompt/status lines, put the terminal in a sensible mode,
    /// and otherwise clean up everything we've done to the terminal. This will
    /// happen automatically when this `IO` instance is dropped, so you
    /// probably don't need to call this manually.
    pub fn blocking_die(mut self) {
        self.actually_blocking_die()
    }
    fn report_death(&mut self) {
        self.death_count = self.death_count.saturating_add(1);
        if self.death_count >= MAX_DEATH_COUNT {
            panic!("Client program is looping forever despite receiving `Response::Dead` {} times. Program bug!", MAX_DEATH_COUNT);
        }
    }
    /// Read a [`Response`](enum.Response.html) from the user, blocking this
    /// task until something is received.
    ///
    /// This is an asynchronous function. To read from non-asynchronous code,
    /// you should use `blocking_read` instead.
    ///
    /// If `Response::Dead` is received too many times, Liso will assume your
    /// program is ignoring it and panic! Avoid this problem by handling
    /// `Response::Dead` correctly.
    pub async fn read(&mut self) -> Response {
        match self.rx.recv().await {
            None => { self.report_death(); Response::Dead },
            Some(x) => x,
        }
    }
    /// Read a [`Response`](enum.Response.html) from the user, blocking this
    /// thread until the given `timeout` elapses or something is received.
    ///
    /// This is a synchronous function. To achieve the same effect
    /// asynchronously, you can wrap `read` in `tokio::time::timeout`.
    ///
    /// If `Response::Dead` is received too many times, Liso will assume your
    /// program is ignoring it and panic! Avoid this problem by handling
    /// `Response::Dead` correctly.
    pub fn read_timeout(&mut self, timeout: Duration) -> Option<Response> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().expect("Couldn't create temporary Tokio runtime for `read_timeout`");
        rt.block_on(async {
            let timeout = tokio::time::timeout(timeout, self.rx.recv());
            match timeout.await {
                Ok(None) => { self.report_death(); Some(Response::Dead) },
                Ok(Some(x)) => Some(x),
                Err(_) => None,
            }
        })
    }
    /// Read a [`Response`](enum.Response.html) from the user, blocking this
    /// thread until the given `deadline` is reached or something is received.
    ///
    /// This is a synchronous function. To achieve the same effect
    /// asynchronously, you can wrap `read` in `tokio::time::timeout_at`.
    ///
    /// If `Response::Dead` is received too many times, Liso will assume your
    /// program is ignoring it and panic! Avoid this problem by handling
    /// `Response::Dead` correctly.
    pub fn read_deadline(&mut self, deadline: Instant) -> Option<Response> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().expect("Couldn't create temporary Tokio runtime for `read_deadline`");
        rt.block_on(async {
            let timeout = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), self.rx.recv());
            match timeout.await {
                Ok(None) => { self.report_death(); Some(Response::Dead) },
                Ok(Some(x)) => Some(x),
                Err(_) => None,
            }
        })
    }
    /// Read a [`Response`](enum.Response.html) from the user, blocking this
    /// thread until something is received.
    ///
    /// This is a synchronous function. To read from asynchronous code, you
    /// should use `read` instead.
    ///
    /// If `Response::Dead` is received too many times, Liso will assume your
    /// program is ignoring it and panic! Avoid this problem by handling
    /// `Response::Dead` correctly.
    pub fn blocking_read(&mut self) -> Response {
        match self.rx.blocking_recv() {
            None => { self.report_death(); Response::Dead },
            Some(x) => x,
        }
    }
    /// Read a [`Response`](enum.Response.html) from the user, if one is
    /// available. If no inputs are currently available, return immediately
    /// instead of blocking or waiting.
    ///
    /// If `Response::Dead` is received too many times, Liso will assume your
    /// program is ignoring it and panic! Avoid this problem by handling
    /// `Response::Dead` correctly.
    pub fn try_read(&mut self) -> Option<Response> {
        use tokio::sync::mpsc::error::TryRecvError;
        match self.rx.try_recv() {
            Ok(x) => Some(x),
            Err(TryRecvError::Disconnected) => { self.report_death(); Some(Response::Dead) },
            Err(TryRecvError::Empty) => None,
        }
    }
}

/// Allows you to iterate over the characters in a [`Line`](struct.Line.html),
/// one at a time, along with their style information. This is returned by
/// [`Line::chars()`](struct.Line.html#method.chars).
pub struct LineCharIterator<'a> {
    line: &'a Line,
    cur_element: usize,
    indices: std::str::CharIndices<'a>,
}

/// A single character from a `Line`, along with its associated style and index
/// information. This is returned by
/// [`LineCharIterator`](struct.LineCharIterator.html).
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

/// Produce an `Option<Color>` from a name or expression. For internal use by
/// the [`liso!`](macro.liso.html) and [`liso_add!`](macro.liso_add.html)
/// macros.
#[macro_export]
#[doc(hidden)]
macro_rules! color {
    (Black) => (Some($crate::Color::Black));
    (Red) => (Some($crate::Color::Red));
    (Green) => (Some($crate::Color::Green));
    (Yellow) => (Some($crate::Color::Yellow));
    (Blue) => (Some($crate::Color::Blue));
    (Cyan) => (Some($crate::Color::Cyan));
    (Magenta) => (Some($crate::Color::Magenta));
    (White) => (Some($crate::Color::White));
    (none) => (None);
    (black) => (Some($crate::Color::Black));
    (red) => (Some($crate::Color::Red));
    (green) => (Some($crate::Color::Green));
    (yellow) => (Some($crate::Color::Yellow));
    (blue) => (Some($crate::Color::Blue));
    (cyan) => (Some($crate::Color::Cyan));
    (magenta) => (Some($crate::Color::Magenta));
    (white) => (Some($crate::Color::White));
    (none) => (None);
    ($other:expr) => ($other);
}

/// Add some pieces to a [`Line`](struct.Line.html). More convenient than
/// calling its methods.
///
/// ```rust
/// # use liso::{liso_add, Line, Style};
/// let mut line_a = Line::new();
/// line_a.add_text("Hello ");
/// line_a.set_style(Style::BOLD);
/// line_a.add_text("World!");
/// let mut line_b = Line::new();
/// liso_add!(line_b, "Hello ", bold, "World!");
/// assert_eq!(line_a, line_b);
/// ```
///
/// Use the [`liso!` macro](macro.liso.html) to build an entire line in a
/// single go. See that macro's documentation for more information on the
/// syntax.
#[macro_export]
macro_rules! liso_add {
    // Reset all style and color
    // `reset`
    ($line:ident, reset, $($rest:tt)*) => {
        $line.reset_all();
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, reset) => {
        $line.reset_all();
    };
    // Set fg/bg color
    // (`fg` | `bg`) `=` <COLOR>
    ($line:ident, fg = $color:tt, $($rest:tt)*) => {
        $line.set_fg_color($crate::color!($color));
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, fg = $color:tt) => {
        $line.set_fg_color($crate::color!($color));
    };
    ($line:ident, bg = $color:tt, $($rest:tt)*) => {
        $line.set_bg_color($crate::color!($color));
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, bg = $color:tt) => {
        $line.set_bg_color($crate::color!($color));
    };
    ($line:ident, fg = $color:expr, $($rest:tt)*) => {
        $line.set_fg_color($color);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, fg = $color:expr) => {
        $line.set_fg_color($color);
    };
    ($line:ident, bg = $color:expr, $($rest:tt)*) => {
        $line.set_bg_color($color);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, bg = $color:expr) => {
        $line.set_bg_color($color);
    };
    // Clear styles
    // `plain`
    ($line:ident, plain $($rest:tt)*) => {
        $line.set_style($crate::Style::PLAIN);
        $crate::liso_add!($line, $($rest)*);
    };
    // SET styles
    // `bold` | `dim` | `underline` | `inverse`
    ($line:ident, bold $($rest:tt)*) => {
        $line.set_style($crate::Style::BOLD);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, dim $($rest:tt)*) => {
        $line.set_style($crate::Style::DIM);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, underline $($rest:tt)*) => {
        $line.set_style($crate::Style::UNDERLINE);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, inverse $($rest:tt)*) => {
        $line.set_style($crate::Style::INVERSE);
        $crate::liso_add!($line, $($rest)*);
    };
    // ADD styles
    // `+` (`bold` | `dim` | `underline` | `inverse`)
    ($line:ident, +bold $($rest:tt)*) => {
        $line.activate_style($crate::Style::BOLD);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, +dim $($rest:tt)*) => {
        $line.activate_style($crate::Style::DIM);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, +underline $($rest:tt)*) => {
        $line.activate_style($crate::Style::UNDERLINE);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, +inverse $($rest:tt)*) => {
        $line.activate_style($crate::Style::INVERSE);
        $crate::liso_add!($line, $($rest)*);
    };
    // REMOVE styles
    // `-` (`bold` | `dim` | `underline` | `inverse`)
    ($line:ident, -bold $($rest:tt)*) => {
        $line.deactivate_style($crate::Style::BOLD);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, -dim $($rest:tt)*) => {
        $line.deactivate_style($crate::Style::DIM);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, -underline $($rest:tt)*) => {
        $line.deactivate_style($crate::Style::UNDERLINE);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, -inverse $($rest:tt)*) => {
        $line.deactivate_style($crate::Style::INVERSE);
        $crate::liso_add!($line, $($rest)*);
    };
    // TOGGLE styles
    // `^` (`bold` | `dim` | `underline` | `inverse`)
    ($line:ident, ^bold $($rest:tt)*) => {
        $line.toggle_style($crate::Style::BOLD);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, ^dim $($rest:tt)*) => {
        $line.toggle_style($crate::Style::DIM);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, ^underline $($rest:tt)*) => {
        $line.toggle_style($crate::Style::UNDERLINE);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, ^inverse $($rest:tt)*) => {
        $line.toggle_style($crate::Style::INVERSE);
        $crate::liso_add!($line, $($rest)*);
    };
    // Anything else: text to output.
    ($line:ident, $expr:expr, $($rest:tt)*) => {
        $line.add_text($expr);
        $crate::liso_add!($line, $($rest)*);
    };
    ($line:ident, $expr:expr) => {
        $line.add_text($expr);
    };
    // Strip double commas
    ($line:ident,, $($rest:tt)*) => {
        $crate::liso_add!($line, $($rest)*);
    };
    // Finish munching
    ($line:ident$(,)*) => {
    };
}

/// Constructs a [`Line`](struct.Line.html) from pieces. More convenient than
/// creating a `Line` and calling its methods.
///
/// You can use the [`liso_add!` macro](macro.liso_add.html) to conveniently
/// add pieces to an existing `Line`.
///
/// Setting style doesn't affect color, and vice versa.
///
/// - `plain`  
///   Clear all styles.
/// - `<style>`  
///   *Set* the style, clearing any other styles.
/// - `+<style>`  
///   Enable this style, leaving other styles unaffected.
/// - `-<style>`  
///   Disable this style, leaving other styles unaffected.
/// - `^<style>`  
///   Toggle this style, leaving other styles unaffected.
/// - `fg = <color>`  
///   Set the foreground color.
/// - `bg = <color>`  
///   Set the background color.
/// - `reset`  
///   Clear all style and color information.
/// - `<text>`  
///   Text to output.
///
/// You have to put a comma after `fg = ...`, `bg = ...`, `reset`, and text.
/// They are optional everywhere else.
///
/// ```rust
/// # use liso::liso;
/// # let error_message = "Hello World!";
/// let line = liso!(fg = red, "Error: ", bold, format!("{}", error_message));
/// let line = liso!("Do you want to proceed? This is a ", bold+underline,
///                  "really", plain, " bad idea!");
/// ```
///
/// `<style>` may be `bold`, `dim`, `inverse`, `italic`, or `plain`.
/// `<color>` may be the actual name of a [`Color`](enum.Color.html), the
/// lowercase equivalent, `None`/`none`, or any expression evaluating to an
/// `Option<Color>`. `<text>` may be anything that you could pass directly to
/// [`Line::add_text()`](struct.Line.html#method.add_text), including a simple
/// string literal or a call to `format!`.
#[macro_export]
macro_rules! liso {
    ($($rest:tt)*) => {
        {
            let mut line = $crate::Line::new();
            $crate::liso_add!(line, $($rest)*,);
            line
        }
    };
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
    const MY_BLUE: Option<Color> = Some(Color::Blue);
    const MY_RED: Option<Color> = Some(Color::Red);
    #[test]
    fn line_macro() {
        let mut line = Line::new();
        line.add_text("This is a test");
        line.set_fg_color(Some(Color::Blue));
        line.add_text(" of BLUE TESTS!");
        line.set_fg_color(Some(Color::Red));
        line.add_text(" And RED TESTS!");
        line.set_bg_color(Some(Color::Blue));
        line.add_text(" Now with backgrounds,");
        line.set_bg_color(Some(Color::Red));
        line.add_text(" and other backgrounds!");
        let alt_line = liso![
            "This is a test",
            fg = Blue,
            " of BLUE TESTS!",
            fg = crate::tests::MY_RED,
            " And RED TESTS!",
            bg = crate::tests::MY_BLUE,
            " Now with backgrounds,",
            bg = red,
            " and other backgrounds!",
        ];
        assert_eq!(line, alt_line);
    }
    #[test] #[cfg(feature="wrap")]
    fn line_wrap() {
        let mut line = liso![
            "This is a simple line wrapping test."
        ];
        line.wrap_to_width(20);
        assert_eq!(line,
                   liso!["This is a simple\nline wrapping test."]);
    }
    #[test] #[cfg(feature="wrap")]
    fn line_wrap_splat() {
        for n in 1 .. 200 {
            let mut line = liso![
                "This is ", bold, "a test", plain, " of line wrapping?"
            ];
            line.wrap_to_width(n);
        }
    }
}
