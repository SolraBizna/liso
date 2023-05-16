//! Liso (LEE-soh) is an acronym for Line Input with Simultaneous Output. It is
//! a library for a particular kind of text-based Rust application; one where
//! the user is expected to give command input at a prompt, but output can
//! occur at any time. It provides simple line editing, and prevents input from
//! clashing with output. It can be used asynchronously (with `tokio`) or
//! synchronously (without).
//!
//! # Usage
//! 
//! Create an [`InputOutput`](struct.InputOutput.html) object with
//! `InputOutput::new()`. Liso will automatically configure itself based on how
//! your program is being used.
//! 
//! Your `InputOutput` instance can be used to send output or receive input.
//! Call `clone_output` to create an [`OutputOnly`](struct.OutputOnly.html)
//! instance, which can only be used to send output. You can call
//! `clone_output` as many times as you like, as well as cloning the
//! `OutputOnly`s directly. An unlimited number of threads or tasks can send
//! output through Liso, but only one thread/task can receive user input:
//! whichever one currently holds the `InputOutput` instance.
//! 
//! If the `global` feature is enabled, which it is by default, then you
//! don't *have* to create `OutputOnly` instances and keep them around in order
//! to send output. See [the "Global" section](#global) for more information.
//!
//! Liso can work with `String`s and `&str`s directly. If you want to add style
//! or color, create a [`Line`](struct.Line.html), either manually or using
//! the convenient [`liso!` macro](macro.liso.html). Send output to the
//! user by calling [`println()`](struct.Output.html#method.println) or
//! [`wrapln()`](struct.Output.html#method.wrapln), whichever you prefer. Any
//! styling and color information is reset after the line is output, so you
//! don't have to worry about dangling attributes.
//! 
//! Liso supports a prompt line, which is presented ahead of the user input.
//! Use [`prompt()`](struct.Output.html#method.prompt) to set it. Styling and
//! color information is *not* reset between the prompt and the current input
//! text, so you can style/color the input text by having the desired
//! styles/colors active at the end of the prompt line.
//! 
//! Liso supports an optional status line, which "hangs out" above the input
//! text. Use [`status()`](struct.Output.html#method.status) to set it. Printed
//! text appears above the status line, the prompt and any in-progress input
//! appears below it. Use this to present contextual or frequently-changing
//! information.
//! 
//! Liso supports "notices", temporary messages that appear in place of the
//! prompt and input for a limited time. Use
//! [`notice()`](struct.Output.html#method.notice) to display one. The notice
//! will disappear when the allotted time elapses, when the user presses any
//! key, or when another notice is displayed, whichever happens first. You
//! should only use this in direct response to user input; in fact, the only
//! legitimate use may be to complain about an unknown control character. (See
//! [`Response`](enum.Response.html) for an example of this use.)
//! 
//! # Global
//!
//! If the `global` feature is enabled (which it is by default), you can call
//! [`output()`](fn.output.html) to get a valid `OutputOnly` instance any time
//! that an `InputOutput` instance is alive. This will panic if there is *not*
//! an `InputOutput` instance alive, so you'll still have to have one.
//!
//! With `global` enabled, you can also use the
//! [`println!`](macro.println.html) or [`wrapln!`](macro.wrapln.html) macros
//! to perform output directly and conveniently. `println!(...)` is equivalent
//! to `output().println!(liso!(...))`.
//!
//! Using the `output()` function, or the `println!`/`wrapln!` macros, is
//! noticeably less efficient than creating an `OutputOnly` instance ahead of
//! time, whether by calling `clone_output()` or by calling `output()` and
//! caching the result. But, it's probably okay as long as you're not hoping to
//! do it hundreds of thousands of times per second.
//!
//! # History
//! 
//! If the `history` feature is enabled (which it is by default), Liso supports
//! a rudimentary command history. It provides a conservative default that
//! isn't backed by any file. Try:
//! 
//! ```rust
//! # let io = liso::InputOutput::new();
//! # let some_path = "DefinitelyDoesNotExist";
//! io.swap_history(liso::History::from_file(some_path).unwrap());
//! ```
//! 
//! to make it backed by a file, and see [`History`](struct.History.html) for
//! more information.
//! 
//! # Completion
//! 
//! If the `completion` feature is enabled (which it is by default), Liso
//! supports tab completion. Implement [`Completor`](trait.Completor.html),
//! then use [`set_completor`](struct.Output.html#method.set_completor) to make
//! your new completor active. See the linked documentation for more
//! information.
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
    any::Any,
    borrow::Cow,
    time::{Duration, Instant},
    sync::mpsc as std_mpsc,
};

#[cfg(not(feature="global"))]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature="history")]
use std::{
    sync::{Arc, RwLock, RwLockReadGuard},
};

#[cfg(feature="completion")]
use std::num::NonZeroU32;

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
#[cfg(unix)] mod unix_util;

#[cfg(feature="history")]
mod history;
#[cfg(feature="history")]
pub use history::*;

#[cfg(feature="completion")]
mod completion;
#[cfg(feature="completion")]
pub use completion::*;

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
///   red, green, and none of the other colors.)
/// - Some users will be using unexpected themes. White on black, black on
///   white, green on black, yellow on orange, and "Solarized" are all common.
/// - Many users have some form of colorblindness. The most common form,
///   affecting as much as 8% of the population, would make `Red`, `Yellow`,
///   and `Green` hard to distinguish from one another. Every other imaginable
///   variation also exists.
/// 
/// And some guidelines to adhere to:
/// 
/// - Never assume you know what color `None` is. It could be white, black, or
///   something entirely unexpected.
/// - Never specify a foreground color of `White` or `Black` without also
///   specifying a background color, or vice versa.
/// - Never specify the same color for both foreground and background at the
///   same time.
/// - Instead of setting white-on-black or black-on-white, consider using
///   [inverse video](struct.Style.html#associatedconstant.INVERSE) to achieve
///   your goal instead.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
#[repr(u8)]
pub enum Color {
    /// Absence of light. The color of space. (Some terminals will render this
    /// as a dark gray instead.)
    Black=0,
    /// The color of blood, danger, and rage.
    Red=1,
    /// The color of plants, safety, and circadian stasis.
    Green=2,
    /// The color of all the worst chemicals.
    Yellow=3,
    /// The color of a calm ocean.
    Blue=4,
    /// The color of a clear sky.
    Cyan=5,
    /// A color that occurs rarely in nature, but often in screenshots of GEM.
    Magenta=6,
    /// A (roughly) equal mix of all wavelengths of light.
    White=7,
}

impl Color {
    // Convert to the equivalent Crossterm color.
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
    // Convert to an Atari ST 16-color palette index (bright).
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
    // Convert to an Atari ST 16-color palette index (dim).
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
    // Convert to the nearest Atari ST 4-color palette index.
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
        /// terminals that don't support color *do* support this.)
        ///
        /// Liso toggles this whenever a control sequence is inserted into a
        /// [`Line`](struct.Line.html):
        ///
        /// ```rust
        /// # use liso::liso;
        /// assert_eq!(liso!("Type \x03 to quit."),
        ///            liso!("Type ", ^inverse, "^C", ^inverse, " to quit."));
        const INVERSE = 1 << 3;
        /// An alias for [`INVERSE`](#associatedconstant.INVERSE). I prefer to
        /// use the term "inverse video" rather than "reverse video", as the
        /// latter might be confused for some kind of "mirrored video" feature.
        #[doc(alias="INVERSE")]
        const REVERSE = 1 << 3;
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

/// This struct contains all the methods that the
/// [`OutputOnly`](struct.OutputOnly.html) and
/// [`InputOutput`](struct.InputOutput.html) structs have in common. Any method
/// of this struct may be called on either of the other structs.
pub struct Output {
    tx: std_mpsc::Sender<Request>,
}

/// Sends output to the terminal. You can have more than one of these, shared
/// freely among threads and tasks. Give one to every thread, task, or object
/// that needs to produce output.
pub struct OutputOnly(Output);

/// Receives input from, and sends output to, the terminal. You can *send
/// output* from any number of threads
/// (see [`Output::clone_output`](struct.Output.html#method.clone_output)), but
/// only one thread at a time may have ownership of the overlying `InputOutput`
/// type and therefore the ability to *receive input*.
pub struct InputOutput {
    output: Output,
    rx: tokio_mpsc::UnboundedReceiver<Response>,
    #[cfg(feature="history")]
    history: Arc<RwLock<History>>,
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
/// display. The [`liso!`](macro.liso.html) macro is extremely convenient for
/// building these. You can also pass a `String`, `&str`, or `Cow<str>` to
/// most Liso functions that accept a `Line`.
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
    /// Adds additional text to the `Line` using the currently-active
    /// [`Style`][1] and [`Color`][2]s..
    ///
    /// You may pass a `String`, `&str`, or `Cow<str>` here, but not a `Line`.
    /// If you want to append styled text, see [`append_line`][3]. If you want
    /// to append the text from a `Line` but discard its style information,
    /// call [`as_str`][4] on that `Line`.
    ///
    /// [1]: struct.Style.html
    /// [2]: enum.Color.html
    /// [3]: #method.append_line
    /// [4]: #method.as_str
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
    /// Returns the currently active [`Style`][1].
    ///
    /// [1]: struct.Style.html
    pub fn get_style(&self) -> Style {
        match self.elements.last() {
            None => Style::PLAIN,
            Some(x) => x.style,
        }
    }
    /// Change the active [`Style`][1] to exactly that given.
    ///
    /// [1]: struct.Style.html
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
    /// Toggle every given [`Style`][1].
    ///
    /// [1]: struct.Style.html
    pub fn toggle_style(&mut self, nu: Style) -> &mut Line {
        let old = self.get_style();
        self.set_style(old ^ nu)
    }
    /// Activate the given [`Style`][1]s, leaving any already-active `Style`s
    /// active.
    ///
    /// [1]: struct.Style.html
    pub fn activate_style(&mut self, nu: Style) -> &mut Line {
        let old = self.get_style();
        self.set_style(old | nu)
    }
    /// Deactivate the given [`Style`][1]s, without touching any unmentioned
    /// `Style`s that were already active.
    ///
    /// [1]: struct.Style.html
    pub fn deactivate_style(&mut self, nu: Style) -> &mut Line {
        let old = self.get_style();
        self.set_style(old - nu)
    }
    /// Deactivate *all* [`Style`][1]s. Same as calling
    /// `set_style(Style::PLAIN)`.
    ///
    /// [1]: struct.Style.html
    pub fn clear_style(&mut self) -> &mut Line {
        self.set_style(Style::PLAIN)
    }
    /// Gets the current [`Color`][1]s, both foreground and background.
    ///
    /// [1]: enum.Color.html
    pub fn get_colors(&self) -> (Option<Color>, Option<Color>) {
        match self.elements.last() {
            None => (None, None),
            Some(x) => (x.fg, x.bg),
        }
    }
    /// Sets the foreground [`Color`][1].
    ///
    /// [1]: enum.Color.html
    pub fn set_fg_color(&mut self, nu: Option<Color>) -> &mut Line {
        let (fg, bg) = self.get_colors();
        if nu != fg { self.set_colors(nu, bg); }
        self
    }
    /// Sets the background [`Color`][1].
    ///
    /// [1]: enum.Color.html
    pub fn set_bg_color(&mut self, nu: Option<Color>) -> &mut Line {
        let (fg, bg) = self.get_colors();
        if nu != bg { self.set_colors(fg, nu); }
        self
    }
    /// Sets both the foreground and background [`Color`][1].
    ///
    /// [1]: enum.Color.html
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
    /// Reset ALL [`Style`][1] and [`Color`][2] information to default.
    /// Equivalent to:
    ///
    /// ```
    /// # use liso::Style;
    /// # let mut line = liso::Line::new();
    /// # liso::liso_add!(line, fg=green, bg=red, underline);
    /// line.set_style(Style::PLAIN).set_colors(None, None);
    /// # assert_eq!(line, liso::liso!(plain, fg=none, bg=none));
    /// ```
    ///
    /// (In fact, that is the body of this function.)
    ///
    /// [1]: struct.Style.html
    /// [2]: enum.Color.html
    pub fn reset_all(&mut self) -> &mut Line {
        self.set_style(Style::PLAIN).set_colors(None, None)
    }
    /// Returns true if this line contains no text. (It may yet contain some
    /// [`Style`][1] or [`Color`][2] information.)
    ///
    /// [1]: struct.Style.html
    /// [2]: enum.Color.html
    pub fn is_empty(&self) -> bool { self.text.is_empty() }
    /// Returns the number of **BYTES** of text this line contains.
    pub fn len(&self) -> usize { self.text.len() }
    /// Iterate over chars of the line, including [`Style`][1] and [`Color`][2]
    /// information, one `char` at a time.
    ///
    /// The usual caveats about the difference between a `char` and a character
    /// apply. Unicode etc.
    ///
    /// Yields: `(byte_index, character, style, fgcolor, bgcolor)`
    ///
    /// [1]: struct.Style.html
    /// [2]: enum.Color.html
    pub fn chars(&self) -> LineCharIterator<'_> {
        LineCharIterator::new(self)
    }
    /// Add a linebreak and then clear [`Style`][1] and [`Color`][2]s.
    ///
    /// Equivalent to:
    ///
    /// ```
    /// # use liso::Style;
    /// # let mut line = liso::Line::new();
    /// # liso::liso_add!(line, fg=green, bg=red, underline);
    /// line.add_text("\n");
    /// line.set_style(Style::empty());
    /// line.set_colors(None, None);
    /// # assert_eq!(line, liso::liso!(fg=green, bg=red, underline,
    /// #   "\n", reset));
    /// ```
    ///
    /// (In fact, that is the body of this function.)
    ///
    /// [1]: struct.Style.html
    /// [2]: enum.Color.html
    pub fn reset_and_break(&mut self) {
        self.add_text("\n");
        self.set_style(Style::empty());
        self.set_colors(None, None);
    }
    /// Append another Line to ourselves, including [`Style`][1] and
    /// [`Color`][2] information. You may want to [`reset_and_break`][3] first.
    ///
    /// [1]: struct.Style.html
    /// [2]: enum.Color.html
    /// [3]: #method.reset_and_break
    pub fn append_line(&mut self, other: &Line) {
        for element in other.elements.iter() {
            self.set_style(element.style);
            self.set_colors(element.fg, element.bg);
            self.add_text(&other.text[element.start .. element.end]);
        }
    }
    /// Insert linebreaks as necessary to make it so that no line within this
    /// `Line` is wider than the given number of columns. Only available with
    /// the `wrap` feature, which is enabled by default.
    ///
    /// Rather than calling this method yourself, you definitely want to use
    /// the [`wrapln`](struct.Output.html#method.wrapln) method instead of the
    /// [`println`](struct.Output.html#method.println) method. That way, Liso
    /// will automatically wrap the line of text to the correct width for the
    /// user's terminal.
    #[cfg(feature="wrap")]
    pub fn wrap_to_width(&mut self, width: usize) {
        assert!(width > 0);
        let newline_positions: Vec<usize>
        = self.text.chars().enumerate().filter_map(|(n,c)| {
            if c == '\n' { Some(n) }
            else { None }
        }).chain(Some(self.text.len())).collect();
        let start_iter = newline_positions.iter().rev()
            .skip(1).map(|x| *x+1).chain(Some(0usize));
        let end_iter = newline_positions.iter().rev();
        for (start, &end) in start_iter.zip(end_iter) {
            if start >= end { continue }
            let wrap_vec = textwrap::wrap(&self.text[start..end], width);
            let mut edit_vec = Vec::with_capacity(wrap_vec.len());
            let mut cur_end = start;
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
                if range.start > 0 && self.text.as_bytes()[range.start-1] == b'\n' { continue }
                self.erase_and_insert_newline(range);
            }
        }
    }
    // Internal use only.
    #[cfg(feature="wrap")]
    fn erase_and_insert_newline(&mut self, range: std::ops::Range<usize>) {
        let delta_bytes = 1 - (range.end as isize - range.start as isize);
        self.text.replace_range(range.clone(), "\n");
        let mut elements_len = self.elements.len();
        let mut i = self.elements.len();
        loop {
            if i == 0 { break }
            i -= 1;
            let element = &mut self.elements[i];
            if element.end >= range.end {
                element.end = ((element.end as isize) + delta_bytes) as usize;
            }
            else if element.end > range.start {
                element.end = range.start;
            }
            if element.start >= range.end {
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
    /// Sent by `echoln`
    OutputEcho(Line),
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
    /// Sent by `suspend_and_run`
    SuspendAndRun(Box<dyn FnMut() + Send>),
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
    /// Used to implement notices.
    Heartbeat,
    /// If the crossterm event system is being used, this is an event received.
    /// This can be the case even if the crossterm *input* system isn't being
    /// used.
    CrosstermEvent(crossterm::event::Event),
    /// Sent by `send_custom`.
    Custom(Box<dyn Any + Send>),
    /// Sent when the `History` is changed.
    #[cfg(feature="history")]
    BumpHistory,
    /// Sent when the `Completor` is to be replaced.
    #[cfg(feature="completion")]
    SetCompletor(Option<Box<dyn Completor>>),
}

/// Input received from the user, or a special condition. Returned by any of
/// the following [`InputOutput`](struct.InputOutput.html) methods:
///
/// - [`read_async`](struct.InputOutput.html#method.read_async) (asynchronous)
/// - [`read_blocking`](struct.InputOutput.html#method.read_blocking)
///   (synchronous, waiting forever)
/// - [`read_timeout`](struct.InputOutput.html#method.read_timeout)
///   (synchronous with timeout)
/// - [`read_deadline`](struct.InputOutput.html#method.read_deadline)
///   (synchronous with deadline)
/// - [`try_read`](struct.InputOutput.html#method.try_read)
///   (polled)
///
/// Example usage:
///
/// ```rust
/// # use liso::{Response, liso};
/// # use std::time::Duration;
/// # let mut io = liso::InputOutput::new();
/// // near the top
/// io.prompt(liso!(fg=green, bold, "> ", reset), true, false);
/// // in your main loop
/// # let response = Response::Input(String::new());
/// # for _ in 0 .. 1 {
/// match response {
///   Response::Input(line) => {
///     io.echoln(liso!(fg=green, dim, "> ", fg=none, &line));
///     match line.as_str() {
///       "hello" => io.println("World!"),
///       "world" => io.println("Hello!"),
///       _ => io.println("何って？"),
///     }
///   },
///   Response::Discarded(line) => {
///     io.echoln(liso!(bold+dim, "X ", -bold, line));
///   },
///   Response::Dead => return,
///   Response::Quit => break,
///   // (handle any other variants you want)
///   other => {
///       io.notice(format!("unknown key {}",
///                         other.as_unknown() as char),
///                 Duration::from_secs(1));
///   },
/// }
/// # break;
/// # }
/// ```
///
/// 
#[derive(Debug)]
#[non_exhaustive]
pub enum Response {
    /// Sent when the user finishes entering a line of input. This is the
    /// entire line. This is the most interesting, and common, variant that
    /// you will receive.
    ///
    /// In case you don't want to do in-depth parsing of the user's input, you
    /// can match against static string literals with a little work. You may
    /// also want to use [`echoln`](struct.Output.html#method.echoln) to echo
    /// the user's input. See the top of this documentation for an example of
    /// both.
    Input(String),
    /// Sent when the terminal or the IO thread have died. Once you receive
    /// this once, you will never receive any other `Response` from Liso again.
    /// Your program should exit soon after, or at the very least should close
    /// down that `InputOutput` instance.
    /// 
    /// If your program receives `Response::Dead` on the same `InputOutput`
    /// instance too many times, Liso will panic. This is to ensure that even
    /// a poorly-written program that ignores `Response::Dead` will still exit
    /// soon after after user input is permanently cut off, whether by a hangup
    /// condition or by a bug in Liso.
    Dead,
    /// Sent when the user types control-C, which normally means they want your
    /// program to quit.
    Quit,
    /// Sent when the user types control-G, discarding their current input. The
    /// passed string is what the state of their input was when they hit
    /// control-G. You should pass this to `echoln`, along with some kind of
    /// feedback that the input was discarded.
    Discarded(String),
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
    /// Sent whenever `send_custom` is called. This can be used to interrupt
    /// the input thread when it's doing a `read_blocking` call.
    Custom(Box<dyn Any + Send>),
    /// Sent when the user presses an unknown control character with the given
    /// value (which will be between 0 and 31 inclusive).
    /// 
    /// Don't use particular values of `Unknown` for any specific purpose.
    /// Later versions of Liso may add additional `Response` variants for new
    /// control keys, or handle more control keys itself, replacing the
    /// `Unknown(...)` values those keys used to send. See the top of this file
    /// for an example of how this variant should be used (i.e. not directly).
    Unknown(u8),
}

impl Response {
    /// Returns the control code that triggered this response, e.g. 10 for
    /// `Input`, 3 for `Quit`, ... Use this to produce a generic "unknown key
    /// key ^X" kind of message for any `Response` variants you don't handle,
    /// perhaps with code like. See the top of this file for an example.
    pub fn as_unknown(&self) -> u8 {
        match self {
            &Response::Input(_) => 10,
            &Response::Discarded(_) => 7,
            &Response::Custom(_) => 0,
            &Response::Quit => 3,
            &Response::Finish => 4,
            &Response::Info => 20,
            &Response::Dead | &Response::Break => 28,
            &Response::Escape => 27,
            &Response::Swap => 24,
            &Response::Unknown(x) => x,
        }
    }
}

impl Output {
    fn send(&self, thing: Request) {
        self.tx.send(thing)
            .expect("Liso output has stopped");
    }
    /// Prints a (possibly styled) line of regular output to the screen.
    ///
    /// Note: As usual with `Output` methods, you can pass a
    /// [`Line`](struct.Line.html), a plain `String`/`&str`, or a `Cow<str>`
    /// here. See also the [`liso!`](macro.liso.html) macro.
    pub fn println<T>(&self, line: T)
    where T: Into<Line> {
        self.send(Request::Output(line.into()))
    }
    /// Prints a (possibly styled) line of regular output to the screen,
    /// wrapping it to the width of the terminal. Only available with the
    /// "wrap" feature, which is enabled by default.
    ///
    /// Note: As usual with `Output` methods, you can pass a
    /// [`Line`](struct.Line.html), a plain `String`/`&str`, or a `Cow<str>`
    /// here. See also the [`liso!`](macro.liso.html) macro.
    pub fn wrapln<T>(&self, line: T)
    where T: Into<Line> {
        self.send(Request::OutputWrapped(line.into()))
    }
    /// Prints a (possibly styled) line of regular output to the screen, but
    /// only if we are being run interactively. Use this if you want to to echo
    /// commands entered by the user, so that echoed commands will not gum up
    /// the output when we are outputting to a pipe.
    ///
    /// Note: As usual with `Output` methods, you can pass a
    /// [`Line`](struct.Line.html), a plain `String`/`&str`, or a `Cow<str>`
    /// here. See also the [`liso!`](macro.liso.html) macro.
    pub fn echoln<T>(&self, line: T)
    where T: Into<Line> {
        self.send(Request::OutputEcho(line.into()))
    }
    /// Sets the status line to the given (possibly styled) text. This will be
    /// displayed above the prompt, but below printed output. (Does nothing in
    /// pipe mode.)
    ///
    /// Note: `status(Some(""))` and `status(None)` will have different
    /// results! The former will display a *blank* status line, while the
    /// latter will display *no* status line.
    ///
    /// Note: As usual with `Output` methods, you can pass a
    /// [`Line`](struct.Line.html), a plain `String`/`&str`, or a `Cow<str>`
    /// here. See also the [`liso!`](macro.liso.html) macro.
    pub fn status<T>(&self, line: Option<T>)
    where T: Into<Line> {
        self.send(Request::Status(line.map(T::into)))
    }
    /// Displays a (possibly styled) notice that temporarily replaces the
    /// prompt. The notice will disappear when the allotted time elapses, when
    /// the user presses any key, or when another notice is displayed,
    /// whichever happens first. (Does nothing in pipe mode.)
    ///
    /// You should only use this in direct response to user input; in fact, the
    /// only legitimate use may be to complain about an unknown control
    /// character. (See [`Response`][1] for an example of this use.)
    ///
    /// Note: As usual with `Output` methods, you can pass a
    /// [`Line`](struct.Line.html), a plain `String`/`&str`, or a `Cow<str>`
    /// here. See also the [`liso!`](macro.liso.html) macro.
    ///
    /// [1]: enum.Response.html
    pub fn notice<T>(&self, line: T, max_duration: Duration)
    where T: Into<Line> {
        self.send(Request::Notice(line.into(), max_duration))
    }
    /// Sets the prompt to the given (possibly styled) text. The prompt is
    /// displayed in front of the user's input, unless we are running in pipe
    /// mode.
    ///
    /// The default prompt is blank, with input allowed.
    ///
    /// - `input_allowed`: True if the user should be allowed to write input.
    /// - `clear_input`: True if any existing partial input should be cleared
    ///   when the new prompt is displayed. (If `input_allowed` is false, this
    ///   should probably be `true`.)
    ///
    /// Note: If the prompt is styled, whatever style is active at the end of
    /// the prompt will be used when displaying the user's input. This is the
    /// only circumstance in which Liso will not automatically reset style
    /// information for you at the end of a `Line`.
    ///
    /// Note: When running in pipe mode, input is always allowed, there is no
    /// way to clear buffered input, and prompts are never displayed. In short,
    /// this function does nothing at all in pipe mode.
    ///
    /// Note: As usual with `Output` methods, you can pass a
    /// [`Line`](struct.Line.html), a plain `String`/`&str`, or a `Cow<str>`
    /// here. See also the [`liso!`](macro.liso.html) macro.
    pub fn prompt<T>(&self, line: T,
                     input_allowed: bool, clear_input: bool)
    where T: Into<Line> {
        let line: Line = line.into();
        self.send(Request::Prompt {
            line: if line.elements.len() == 0 { None } else { Some(line) },
            input_allowed, clear_input
        })
    }
    /// Removes the prompt. The boolean parameters have the same meaning as for
    /// `prompt`.
    #[deprecated="Use `prompt` with a blank line instead."]
    #[doc(hidden)]
    pub fn remove_prompt(&self, input_allowed: bool, clear_input: bool) {
        self.send(Request::Prompt {
            line: None, input_allowed, clear_input
        })
    }
    /// Get the user's attention with an audible or visible bell.
    pub fn bell(&self) {
        self.send(Request::Bell)
    }
    /// Use this when you need to perform some work that outputs directly to
    /// stdout/stderr and can't run it through Liso. Prompt, status, and input
    /// in progress will be erased from the screen, and the terminal will be
    /// put back into normal mode. When the function returns, Liso will set up
    /// the terminal, display the prompt, and continue as normal.
    ///
    /// Bear in mind that this will run in a separate thread, possibly after a
    /// short delay. If you need to return a value, wait for completion, or
    /// otherwise communicate with the main program, you should use the usual
    /// inter-thread communication primitives, such as channels or atomics.
    ///
    /// Note that you **cannot** use this to create a subprocess that will read
    /// from stdin! Even though *output* is suspended, Liso will still be
    /// reading from stdin in another thread, and thus, will be competing with
    /// the subprocess for user input. (On sane UNIXes, this will result in
    /// your program being suspended by your shell, and then misbehaving when
    /// it resumes.) If you want to create a subprocess that can use stdin and
    /// stdout, you'll have to write your own pipe handling based around Liso.
    /// If you want to create a subprocess that can interactively use the
    /// terminal—you have to drop the `InputOutput` instance, and all of the
    /// existing `Output` instances will go dead as a result. Just don't do it!
    pub fn suspend_and_run<F: 'static + FnMut() + Send>(&self, f: F) {
        self.send(Request::SuspendAndRun(Box::new(f)))
    }
    /// Make a new `OutputOnly` that can also output to the terminal. The clone
    /// and the original can be stored in separate places, even in different
    /// threads or tasks. All output will go to the same terminal, without any
    /// conflict between other threads doing output simultaneously or with user
    /// input.
    ///
    /// For `OutputOnly`, this is the same as `clone`. For `InputOutput`, you
    /// must call this method instead, as this makes it clear that you are not
    /// trying to clone the `Input` half of that `InputOutput`.
    pub fn clone_output(&self) -> OutputOnly {
        OutputOnly(Output { tx: self.tx.clone() })
    }
    #[deprecated="Use `clone_output` instead."]
    #[doc(hidden)]
    pub fn clone_sender(&self) -> OutputOnly {
        self.clone_output()
    }
    /// Send the given value to the input thread, wrapped in a
    /// [`Response::Custom`](enum.Response.html#variant.Custom).
    pub fn send_custom<T: Any + Send>(&self, value: T) {
        self.send(Request::Custom(Box::new(value)))
    }
    /// Send the given already-boxed value to the input thread, wrapped in a
    /// [`Response::Custom`](enum.Response.html#variant.Custom).
    pub fn send_custom_box(&self, value: Box<dyn Any + Send>) {
        self.send(Request::Custom(value))
    }
    /// Provide a new `Completor` for doing tab completion.
    #[cfg(feature="completion")]
    pub fn set_completor(&self, completor: Option<Box<dyn Completor>>) {
        self.send(Request::SetCompletor(completor))
    }
}

impl Drop for InputOutput {
    fn drop(&mut self) {
        #[cfg(feature="global")]
        { *LISO_OUTPUT_TX.lock() = None; }
        self.actually_blocking_die();
        #[cfg(not(feature="global"))]
        LISO_IS_ACTIVE.store(false, Ordering::Release);
    }
}

impl core::ops::Deref for InputOutput {
    type Target = Output;
    fn deref(&self) -> &Output { &self.output }
}

impl InputOutput {
    pub fn new() -> InputOutput {
        let we_are_alone;
        #[cfg(feature="global")]
        let mut global_lock = LISO_OUTPUT_TX.lock();
        #[cfg(feature="global")]
        { we_are_alone = global_lock.is_none(); }
        #[cfg(not(feature="global"))]
        match LISO_IS_ACTIVE.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => we_are_alone = true,
            Err(_) => we_are_alone = false,
        }
        if !we_are_alone {
            panic!("Tried to have multiple `liso::InputOutput` instances \
                        active at the same time!")
        }
        let (request_tx, request_rx) = std_mpsc::channel();
        let (response_tx, response_rx) = tokio_mpsc::unbounded_channel();
        let request_tx_clone = request_tx.clone();
        let history = Arc::new(RwLock::new(History::new()));
        let history_clone = history.clone();
        std::thread::Builder::new().name("Liso output thread".to_owned())
            .spawn(move || {
                #[cfg(feature="history")] 
                let _ =
                    worker::worker(request_tx_clone, request_rx, response_tx, history_clone);
                #[cfg(not(feature="history"))] 
                let _ =
                    worker::worker(request_tx_clone, request_rx, response_tx);
            })
            .unwrap();
        #[cfg(feature="global")]
        { *global_lock = Some(request_tx.clone()); }
        InputOutput {
            output: Output { tx: request_tx },
            rx: response_rx,
            death_count: 0,
            #[cfg(feature="history")]
            history,
        }
    }
    /// Erase the prompt/status lines, put the terminal in a sensible mode,
    /// and otherwise clean up everything we've done to the terminal. This will
    /// happen automatically when this `InputOutput` instance is dropped; you
    /// only need this method if you want to shut Liso down asynchronously for
    /// some reason.
    ///
    /// If `Output`s cloned from this `InputOutput` exist, they will be "dead";
    /// calling their methods will panic!
    pub async fn die(mut self) {
        if self.output.tx.send(Request::Die).is_err() {
            // already dead!
            return
        }
        loop {
            match self.read_async().await {
                Response::Dead => break,
                _ => (),
            }
        }
    }
    fn actually_blocking_die(&mut self) {
        if self.output.tx.send(Request::Die).is_err() {
            // already dead!
            return
        }
        loop {
            match self.try_read() {
                None => std::thread::yield_now(),
                Some(Response::Dead) => break,
                _ => (),
            }
        }
    }
    /// Erase the prompt/status lines, put the terminal in a sensible mode,
    /// and otherwise clean up everything we've done to the terminal. This will
    /// happen automatically when this `InputOutput` instance is dropped, so
    /// you probably don't need to call this manually.
    ///
    /// If `OutputOnly`s cloned from this `InputOutput` exist, they will be
    /// "dead"; calling their methods will panic!
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
    /// you should use `read_blocking` instead.
    ///
    /// If `Response::Dead` is received too many times, Liso will assume your
    /// program is ignoring it and panic! Avoid this problem by handling
    /// `Response::Dead` correctly.
    pub async fn read_async(&mut self) -> Response {
        match self.rx.recv().await {
            None => { self.report_death(); Response::Dead },
            Some(x) => x,
        }
    }
    #[deprecated="Use `read_async` instead."]
    #[doc(hidden)]
    pub async fn read(&mut self) -> Response {
        self.read_async().await
    }
    /// Read a [`Response`](enum.Response.html) from the user, blocking this
    /// thread until the given `timeout` elapses or something is received.
    ///
    /// This is a synchronous function. To achieve the same effect
    /// asynchronously, you can wrap `read_async` in `tokio::time::timeout`.
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
    /// asynchronously, you can wrap `read_async` in `tokio::time::timeout_at`.
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
    /// should use `read_async` instead.
    ///
    /// If `Response::Dead` is received too many times, Liso will assume your
    /// program is ignoring it and panic! Avoid this problem by handling
    /// `Response::Dead` correctly.
    pub fn read_blocking(&mut self) -> Response {
        match self.rx.blocking_recv() {
            None => { self.report_death(); Response::Dead },
            Some(x) => x,
        }
    }
    #[deprecated="Use `read_blocking` instead."]
    #[doc(hidden)]
    pub fn blocking_read(&mut self) -> Response {
        self.read_blocking()
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
    /// Provide a new `History` for Liso to use. Returns the old `History`
    /// instance.
    #[cfg(feature="history")]
    pub fn swap_history(&self, mut history: History) -> History {
        let mut lock = self.history.write().unwrap();
        std::mem::swap(&mut history, &mut *lock);
        drop(lock);
        let _ = self.tx.send(Request::BumpHistory);
        history
    }
    /// Lock the `History` for reading and return a reference to it. Make it
    /// brief!
    #[cfg(feature="history")]
    pub fn read_history(&self) -> RwLockReadGuard<History> {
        self.history.read().unwrap()
    }
}

/// Allows you to iterate over the characters in a [`Line`](struct.Line.html),
/// one at a time, along with their [`Style`][1] and [`Color`][2] information.
/// This is returned by [`Line::chars()`](struct.Line.html#method.chars).
///
/// [1]: struct.Style.html
/// [2]: enum.Color.html
pub struct LineCharIterator<'a> {
    line: &'a Line,
    cur_element: usize,
    indices: std::str::CharIndices<'a>,
}

/// A single character from a `Line`, along with the byte index it begins at,
/// and the [`Style`][1] and [`Color`][2]s it would be displayed with. This is
/// yielded by [`LineCharIterator`](struct.LineCharIterator.html), which is
/// returned by [`Line::chars()`](struct.Line.html#method.chars).
///
/// [1]: struct.Style.html
/// [2]: enum.Color.html
#[derive(Clone,Copy,Debug)]
pub struct LineChar {
    /// Byte index within the `Line` of the first byte of this `char`.
    pub index: usize,
    /// The actual `char`. This is an individual Unicode code point. *Most*
    /// code points correspond to single *characters*, but some are combining
    /// characters (which change the rendering of nearby printable characters),
    /// and some are invisible. And even the code points that are single
    /// *characters* don't correspond to single *graphemes*. (This uncertainty
    /// applies to all uses of Unicode, including other places in Rust where
    /// you have a `char`.)
    pub ch: char,
    /// [`Style`](struct.Style.html) (bold, inverse, etc.) that would be used
    /// to display this `char`.
    pub style: Style,
    /// Foreground [`Color`](enum.Color.html) that would be used to display
    /// this `char`.
    pub fg: Option<Color>,
    /// Background [`Color`](enum.Color.html) that would be used to display
    /// this `char`.
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
    /// printed in the style of both `LineChar`s, false if it might be possible
    /// to distinguish them. Used to optimize endfill when overwriting one line
    /// with another. You probably don't need this method, but in case you do,
    /// here it is.
    ///
    /// In cases whether the answer depends on the specific terminal, returns
    /// false, to be safe. One example is going from inverse video with a
    /// foreground color to non-inverse video with the corresponding background
    /// color. (Some terminals will display the same color differently
    /// depending on whether it's foreground or background, and some of those
    /// terminals implement inverse by simply swapping foreground and
    /// background, therefore we can't count on them looking the same just
    /// because the color indices are the same.)
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

impl core::ops::Deref for OutputOnly {
    type Target = Output;
    fn deref(&self) -> &Output { &self.0 }
}

impl Clone for OutputOnly {
    fn clone(&self) -> OutputOnly { self.clone_output() }
}

#[cfg(feature="wrap")]
fn convert_subset_slice_to_range(outer: &str, inner: &str) -> (usize, usize) {
    if inner.len() == 0 { return (0, 0) }
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
    ($line:ident, reverse $($rest:tt)*) => {
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
    ($line:ident, +reverse $($rest:tt)*) => {
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
    ($line:ident, -reverse $($rest:tt)*) => {
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
    ($line:ident, ^reverse $($rest:tt)*) => {
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
/// `<style>` may be `bold`, `dim`, `inverse`/`reverse`, `italic`, or `plain`.
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
    #[test] #[cfg(feature="wrap")]
    fn lange_wrap() {
        let mut line = liso![
            "This is a simple line wrapping test.\n\nIt has two newlines in it."
        ];
        line.wrap_to_width(20);
        assert_eq!(line,
                   liso!["This is a simple\nline wrapping test.\n\nIt has two newlines\nin it."]);
    }
    #[test] #[cfg(feature="wrap")]
    fn sehr_lagne_wrap() {
        const UNWRAPPED: &str = r#"Mike House was Gegory Houses' borther. He was a world renounced doctor from England, London. His arm was cut off in a fetal MIR incident so he had to walk around with a segway. When he leaned forward, the segway would go real fast. One day, Mike House had a new case for his crack team of other doctors that were pretty good, but not as good as Mike House. So Mike House told them, "WE HAVE A NEW CASE!" And the team said, "ALRIGHT!" And then Mike House said, "IF WE DO NOT SAVE HIM, HE WILL DIE!""#;
        const WRAPPED: &str = r#"Mike House was
Gegory Houses'
borther. He was
a world renounced
doctor from England,
London. His arm was
cut off in a fetal
MIR incident so he
had to walk around
with a segway. When
he leaned forward,
the segway would
go real fast. One
day, Mike House
had a new case for
his crack team of
other doctors that
were pretty good,
but not as good as
Mike House. So Mike
House told them, "WE
HAVE A NEW CASE!"
And the team said,
"ALRIGHT!" And then
Mike House said, "IF
WE DO NOT SAVE HIM,
HE WILL DIE!""#;
        let mut line = Line::from_str(UNWRAPPED);
        line.wrap_to_width(20);
        assert_eq!(line.text, WRAPPED);
        assert_eq!(line.elements.last().unwrap().end, line.text.len());
    }
    #[test] #[cfg(feature="wrap")]
    fn non_synthetic_wrap() {
        let src_line = liso!(bold, fg=yellow, "WARNING: ", reset, "\"/home/sbizna/././././././././nobackup/eph/deleteme/d\" and \"/home/sbizna/././././././././nobackup/eph/deleteme/b\" were identical, but will have differing permissions!");
        let dst_line = liso!(bold, fg=yellow, "WARNING: ", reset, "\"/home/sbizna/././././././././nobackup/eph/deleteme/d\" and \"/home/\nsbizna/././././././././nobackup/eph/deleteme/b\" were identical, but will have\ndiffering permissions!");
        let mut line = src_line.clone();
        line.wrap_to_width(80);
        assert_eq!(line, dst_line);
    }
}

#[deprecated="This type was renamed to `InputOutput` to improve clarity.\n\
              To continue using this name without warnings, try `use \
              liso::InputOutput as IO;`"]
#[doc(hidden)]
pub type IO = InputOutput;
#[deprecated="This type was split into `Output` and `OutputOnly` to improve \
              clarity.\nReplace with `&Output` or `OutputOnly` as needed."]
#[doc(hidden)]
pub type Sender = OutputOnly;

#[cfg(not(feature="global"))]
/// Used to prevent multiple Liso instances from being active at once.
static LISO_IS_ACTIVE: AtomicBool = AtomicBool::new(false);

#[cfg(feature="global")]
static LISO_OUTPUT_TX: parking_lot::Mutex<Option<std_mpsc::Sender<Request>>> = parking_lot::Mutex::new(None);

/// If the `global` feature is enabled (which it is by default), and there is
/// an [`InputOutput`](struct.InputOutput.html) alive somewhere, you can call
/// `output()` to get an [`OutputOnly`](struct.OutputOnly.html) struct that you
/// can use to perform output on it. This is less efficient than creating an
/// `OutputOnly` directly with `clone_output()` and keeping it around, but it
/// is more convenient.
///
/// Calling `output()` when there is no
/// `InputOutput` alive will result in a panic.
#[cfg(feature="global")]
pub fn output() -> OutputOnly {
    match &*LISO_OUTPUT_TX.lock() {
        None => panic!("liso::output() called with no liso::InputOutput alive"),
        Some(x) => OutputOnly(Output { tx: x.clone() }),
    }
}

/// If the `global` feature is enabled (which it is by default), you can use
/// `println!(...)` as convenient shorthand for `output().println(liso!(...))`.
/// This is less efficient than creating an `OutputOnly` with `clone_output()`
/// and keeping it around, but it is more convenient. You will have to
/// explicitly `use liso::println;`, or call it by its full path
/// (`liso::println!`) or Rust may be uncertain whether you meant to use this
/// or `std::println!`. **Panics if there is no `InputOutput` instance alive.**
///
/// Syntax is the same as the [`liso!`](macro.liso.html) macro.
#[cfg(feature="global")]
#[macro_export]
macro_rules! println {
    ($($rest:tt)*) => {
        $crate::output().println(liso!($($rest)*))
    }
}

/// If the `global` and `wrap` features are enabled (which they are by
/// default), you can use `wrapln!(...)` as convenient shorthand for
/// `output().println(liso!(...))`. This is less efficient than creating an
/// `OutputOnly` with `clone_output()` and keeping it around, but it is more
/// convenient. **Panics if there is no `InputOutput` instance alive.**
///
/// Syntax is the same as the [`liso!`](macro.liso.html) macro.
#[cfg(all(feature="global", feature="wrap"))]
#[macro_export]
macro_rules! wrapln {
    ($($rest:tt)*) => {
        $crate::output().wrapln(liso!($($rest)*))
    }
}
