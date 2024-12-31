use super::*;

/// An individual styled span within a line.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineElement {
    /// The style in effect.
    pub(crate) style: Style,
    /// The foreground color (if any).
    pub(crate) fg: Option<Color>,
    /// The background color (if any).
    pub(crate) bg: Option<Color>,
    /// The start (inclusive) and end (exclusive) range of text within the
    /// parent `Line` to which these attributes apply.
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// This is a line of text, with optional styling information, ready for
/// display. The [`liso!`](macro.liso.html) macro is extremely convenient for
/// building these. You can also pass a `String`, `&str`, or `Cow<str>` to
/// most Liso functions that accept a `Line`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub(crate) text: String,
    pub(crate) elements: Vec<LineElement>,
}

impl Line {
    /// Creates a new, empty line.
    pub fn new() -> Line {
        Line {
            text: String::new(),
            elements: Vec::new(),
        }
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
    ///
    /// Unlike the one from the `FromStr` trait, this function always succeeds.
    #[allow(clippy::should_implement_trait)]
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
        if i.len() == 0 {
            return;
        }
        if self.text.is_empty() {
            // The line didn't have any text or elements yet.
            match self.elements.last_mut() {
                None => {
                    self.elements.push(LineElement {
                        style: Style::PLAIN,
                        fg: None,
                        bg: None,
                        start: 0,
                        end: i.len(),
                    });
                }
                Some(x) => {
                    assert_eq!(x.start, 0);
                    assert_eq!(x.end, 0);
                    x.end = i.len();
                }
            }
            self.text = i.into_owned();
        } else {
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
    where
        T: Into<Cow<'a, str>>,
    {
        let i: Cow<str> = i.into();
        if i.len() == 0 {
            return self;
        }
        // we regard as a control character anything in the C0 and C1 control
        // character blocks, as well as the U+2028 LINE SEPARATOR and
        // U+2029 PARAGRAPH SEPARATOR characters. Except newliso!
        let mut control_iterator = i.match_indices(|x: char| {
            (x.is_control() && x != '\n') || x == '\u{2028}' || x == '\u{2029}'
        });
        let first_control_pos = control_iterator.next();
        match first_control_pos {
            None => {
                // No control characters to expand. Put it in directly.
                self.append_text(i);
            }
            Some(mut pos) => {
                let mut plain_start = 0;
                loop {
                    if pos.0 != plain_start {
                        self.append_text(Cow::Borrowed(
                            &i[plain_start..pos.0],
                        ));
                    }
                    let control_char = pos.1.chars().next().unwrap();
                    self.toggle_style(Style::INVERSE);
                    let control_char = control_char as u32;
                    let addendum = if control_char < 32 {
                        format!("^{}", (b'@' + (control_char as u8)) as char)
                    } else {
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
            }
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
            }
            Some(x) => {
                // case 2: no change to attributes
                if x.style == nu {
                    return self;
                }
                // case 3: last element doesn't have text yet.
                else if x.start == x.end {
                    x.style = nu;
                    return self;
                }
                (x.fg, x.bg)
            }
        };
        // (case 1 fall through, or...)
        // case 4: an element with text is here.
        self.elements.push(LineElement {
            style: nu,
            fg,
            bg,
            start: self.text.len(),
            end: self.text.len(),
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
        if nu != fg {
            self.set_colors(nu, bg);
        }
        self
    }
    /// Sets the background [`Color`][1].
    ///
    /// [1]: enum.Color.html
    pub fn set_bg_color(&mut self, nu: Option<Color>) -> &mut Line {
        let (fg, bg) = self.get_colors();
        if nu != bg {
            self.set_colors(fg, nu);
        }
        self
    }
    /// Sets both the foreground and background [`Color`][1].
    ///
    /// [1]: enum.Color.html
    pub fn set_colors(
        &mut self,
        fg: Option<Color>,
        bg: Option<Color>,
    ) -> &mut Line {
        let prev_style = match self.elements.last_mut() {
            // case 1: no elements yet, make one.
            None => Style::PLAIN,
            Some(x) => {
                // case 2: no change to style
                if x.fg == fg && x.bg == bg {
                    return self;
                }
                // case 3: last element doesn't have text yet.
                else if x.start == x.end {
                    x.fg = fg;
                    x.bg = bg;
                    return self;
                }
                x.style
            }
        };
        // (case 1 fall through, or...)
        // case 3: an element with text is here.
        self.elements.push(LineElement {
            style: prev_style,
            fg,
            bg,
            start: self.text.len(),
            end: self.text.len(),
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
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    /// Returns the number of **BYTES** of text this line contains.
    pub fn len(&self) -> usize {
        self.text.len()
    }
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
            self.add_text(&other.text[element.start..element.end]);
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
    #[cfg(feature = "wrap")]
    pub fn wrap_to_width(&mut self, width: usize) {
        assert!(width > 0);
        let newline_positions: Vec<usize> = self
            .text
            .chars()
            .enumerate()
            .filter_map(|(n, c)| if c == '\n' { Some(n) } else { None })
            .chain(Some(self.text.len()))
            .collect();
        let start_iter = newline_positions
            .iter()
            .rev()
            .skip(1)
            .map(|x| *x + 1)
            .chain(Some(0usize));
        let end_iter = newline_positions.iter().rev();
        for (start, &end) in start_iter.zip(end_iter) {
            if start >= end {
                continue;
            }
            let wrap_vec = textwrap::wrap(&self.text[start..end], width);
            let mut edit_vec = Vec::with_capacity(wrap_vec.len());
            let mut cur_end = start;
            for el in wrap_vec.into_iter() {
                // We're pretty sure we didn't use any features that would require
                // an owned Cow. In fact, if we're wrong, the whole feature won't
                // work.
                let slice = match el {
                    Cow::Borrowed(x) => x,
                    Cow::Owned(_) => {
                        panic!("We needed textwrap to do borrows only!")
                    }
                };
                let (start, end) =
                    convert_subset_slice_to_range(&self.text, slice);
                debug_assert!(start <= end);
                if start == end {
                    continue;
                }
                assert!(start >= cur_end);
                if start != 0 {
                    edit_vec.push(cur_end..start);
                }
                cur_end = end;
            }
            for range in edit_vec.into_iter().rev() {
                if range.start > 0
                    && self.text.as_bytes()[range.start - 1] == b'\n'
                {
                    continue;
                }
                self.erase_and_insert_newline(range);
            }
        }
    }
    // Internal use only.
    #[cfg(feature = "wrap")]
    fn erase_and_insert_newline(&mut self, range: std::ops::Range<usize>) {
        let delta_bytes = 1 - (range.end as isize - range.start as isize);
        self.text.replace_range(range.clone(), "\n");
        let mut elements_len = self.elements.len();
        let mut i = self.elements.len();
        loop {
            if i == 0 {
                break;
            }
            i -= 1;
            let element = &mut self.elements[i];
            if element.end >= range.end {
                element.end = ((element.end as isize) + delta_bytes) as usize;
            } else if element.end > range.start {
                element.end = range.start;
            }
            if element.start >= range.end {
                element.start =
                    ((element.start as isize) + delta_bytes) as usize;
            } else if element.start > range.start {
                element.start = range.start;
            }
            if element.end <= element.start {
                if i == elements_len - 1 {
                    // preserve the last element, even if empty
                    element.end = element.start;
                } else {
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

impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}

impl From<String> for Line {
    fn from(val: String) -> Self {
        Line::from_string(val)
    }
}

impl From<&str> for Line {
    fn from(val: &str) -> Self {
        Line::from_str(val)
    }
}

impl From<Cow<'_, str>> for Line {
    fn from(val: Cow<'_, str>) -> Self {
        Line::from_cow(val)
    }
}

impl FromStr for Line {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Line::from_str(s))
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
#[derive(Clone, Copy, Debug)]
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
        self.ch == other.ch
            && self.style == other.style
            && self.fg == other.fg
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
        if a_underline != b_underline {
            return false;
        }
        debug_assert_eq!(a_underline, b_underline);
        let a_inverse = self.style.contains(Style::INVERSE);
        let b_inverse = other.style.contains(Style::INVERSE);
        if a_inverse != b_inverse {
            false
        } else if a_inverse {
            debug_assert!(b_inverse);
            if a_underline && self.bg != other.bg {
                return false;
            }
            self.fg == other.fg
        } else {
            debug_assert!(!a_inverse);
            debug_assert!(!b_inverse);
            if a_underline && self.fg != other.fg {
                return false;
            }
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
            && self.line.elements[self.cur_element].end <= index
        {
            self.cur_element += 1;
        }
        // We should never end up with text in the text string that is not
        // covered by an element.
        debug_assert!(self.cur_element < self.line.elements.len());
        let element = &self.line.elements[self.cur_element];
        Some(LineChar {
            index,
            ch,
            style: element.style,
            fg: element.fg,
            bg: element.bg,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_char_splatting() {
        let mut line = Line::new();
        line.add_text(
            "Escape: \u{001B} Some C1 code: \u{008C} \
                       Paragraph separator: \u{2029}",
        );
        assert_eq!(
            line.text,
            "Escape: ^[ Some C1 code: U+008C \
                    Paragraph separator: U+2029"
        );
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
            fg = MY_RED,
            " And RED TESTS!",
            bg = MY_BLUE,
            " Now with backgrounds,",
            bg = red,
            " and other backgrounds!",
        ];
        assert_eq!(line, alt_line);
    }
    #[test]
    #[cfg(feature = "wrap")]
    fn line_wrap() {
        let mut line = liso!["This is a simple line wrapping test."];
        line.wrap_to_width(20);
        assert_eq!(line, liso!["This is a simple\nline wrapping test."]);
    }
    #[test]
    #[cfg(feature = "wrap")]
    fn line_wrap_splat() {
        for n in 1..200 {
            let mut line =
                liso!["This is ", bold, "a test", plain, " of line wrapping?"];
            line.wrap_to_width(n);
        }
    }
    #[test]
    #[cfg(feature = "wrap")]
    fn lange_wrap() {
        let mut line = liso!["This is a simple line wrapping test.\n\nIt has two newlines in it."];
        line.wrap_to_width(20);
        assert_eq!(
            line,
            liso!["This is a simple\nline wrapping test.\n\nIt has two newlines\nin it."]
        );
    }
    #[test]
    #[cfg(feature = "wrap")]
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
    #[test]
    #[cfg(feature = "wrap")]
    fn non_synthetic_wrap() {
        let src_line = liso!(bold, fg=yellow, "WARNING: ", reset, "\"/home/sbizna/././././././././nobackup/eph/deleteme/d\" and \"/home/sbizna/././././././././nobackup/eph/deleteme/b\" were identical, but will have differing permissions!");
        let dst_line = liso!(bold, fg=yellow, "WARNING: ", reset, "\"/home/sbizna/././././././././nobackup/eph/deleteme/d\" and \"/home/\nsbizna/././././././././nobackup/eph/deleteme/b\" were identical, but will have\ndiffering permissions!");
        let mut line = src_line.clone();
        line.wrap_to_width(80);
        assert_eq!(line, dst_line);
    }
}
