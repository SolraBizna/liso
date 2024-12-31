use super::*;

impl Line {
    /// Adds additional text to the `Line`, respecting a subset of ANSI escape
    /// sequences in the process.
    ///
    /// We support only CSI SGR codes (escape followed by `[` ending with `m`).
    /// All unsupported codes are passed through unchanged.
    pub fn add_ansi_text<'a, T>(&mut self, input_line: T) -> &mut Line
    where
        T: Into<Cow<'a, str>>,
    {
        let input_line = input_line.into();
        let mut unescaped_start = 0;
        'outer: while unescaped_start < input_line.len() {
            let mut char_indices =
                input_line[unescaped_start..].char_indices();
            loop {
                let Some((sequence_start, ch)) = char_indices.next() else {
                    break;
                };
                if ch != '\x1B' {
                    continue;
                }
                let Some((_, ch)) = char_indices.next() else {
                    break;
                };
                if ch != '[' {
                    continue;
                }
                if sequence_start != 0 {
                    self.add_text(
                        &input_line[unescaped_start
                            ..unescaped_start + sequence_start],
                    );
                }
                // in case of a parse error
                unescaped_start += sequence_start + 2;
                let csi_bytes = input_line[unescaped_start..].as_bytes();
                let mut i = 0;
                // a CSI command starts with zero or more "parameter bytes"
                let param_start = i;
                while i < csi_bytes.len()
                    && (0x30..=0x3F).contains(&csi_bytes[i])
                {
                    i += 1;
                }
                let param_end = i;
                // followed by zero or more "intermediate bytes"
                let intermediate_start = i;
                while i < csi_bytes.len()
                    && (0x30..=0x3F).contains(&csi_bytes[i])
                {
                    i += 1;
                }
                let intermediate_end = i;
                // followed by one "final byte"
                if csi_bytes.get(i).copied() != Some(b'm')
                    || intermediate_start != intermediate_end
                {
                    // not an SGR code, or not a valid CSI code
                    self.add_text("\x1B[");
                    continue 'outer;
                };
                // now, let the graphic renditions begin!
                let mut codes = input_line[unescaped_start + param_start
                    ..unescaped_start + param_end]
                    .split(';')
                    .map(|code| code.parse::<u32>().unwrap_or(0));
                while let Some(code) = codes.next() {
                    match code {
                        0 => drop(self.reset_all()),
                        1 => drop(self.activate_style(Style::BOLD)),
                        2 => drop(self.activate_style(Style::DIM)),
                        3 => drop(self.activate_style(Style::ITALIC)),
                        4 => drop(self.activate_style(Style::UNDERLINE)),
                        7 => drop(self.activate_style(Style::REVERSE)),
                        21 => drop(self.deactivate_style(Style::BOLD)),
                        22 => drop(
                            self.deactivate_style(Style::BOLD | Style::DIM),
                        ),
                        23 => drop(self.deactivate_style(Style::ITALIC)),
                        24 => drop(self.deactivate_style(Style::UNDERLINE)),
                        27 => drop(self.deactivate_style(Style::REVERSE)),
                        30 => drop(self.set_fg_color(Some(Color::Black))),
                        31 => drop(self.set_fg_color(Some(Color::Red))),
                        32 => drop(self.set_fg_color(Some(Color::Green))),
                        33 => drop(self.set_fg_color(Some(Color::Yellow))),
                        34 => drop(self.set_fg_color(Some(Color::Blue))),
                        35 => drop(self.set_fg_color(Some(Color::Magenta))),
                        36 => drop(self.set_fg_color(Some(Color::Cyan))),
                        37 => drop(self.set_fg_color(Some(Color::White))),
                        39 => drop(self.set_fg_color(None)),
                        40 => drop(self.set_bg_color(Some(Color::Black))),
                        41 => drop(self.set_bg_color(Some(Color::Red))),
                        42 => drop(self.set_bg_color(Some(Color::Green))),
                        43 => drop(self.set_bg_color(Some(Color::Yellow))),
                        44 => drop(self.set_bg_color(Some(Color::Blue))),
                        45 => drop(self.set_bg_color(Some(Color::Magenta))),
                        46 => drop(self.set_bg_color(Some(Color::Cyan))),
                        47 => drop(self.set_bg_color(Some(Color::White))),
                        49 => drop(self.set_bg_color(None)),
                        38 | 48 | 58 => {
                            match codes.next() {
                                Some(5) => {
                                    // 8-bit color, not supported
                                    let _index = codes.next();
                                }
                                Some(2) => {
                                    // RGB color, not supported
                                    let _r = codes.next();
                                    let _g = codes.next();
                                    let _b = codes.next();
                                }
                                _ => (),
                            }
                        }
                        // IGNORE all unknown SGR codes
                        _ => (),
                    };
                }
                // when we resume parsing text, it will be after this SGR code
                unescaped_start += i + 1;
                continue 'outer;
            }
            self.add_text(&input_line[unescaped_start..]);
            break;
        }
        self
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn basic_ansi_test() {
        let mut line = Line::new();
        line.add_ansi_text("Hello \x1B[1;32mWorld\x1B[21m!\x1B[0m Yay!");
        assert_eq!(
            line,
            liso!("Hello ", +bold, fg=green, "World", -bold, "!", reset, " Yay!")
        );
        assert_eq!(
            liso!(ansi "Hello \x1B[1;32mWorld\x1B[21m!\x1B[0m Yay!"),
            liso!("Hello ", +bold, fg=green, "World", -bold, "!", reset, " Yay!")
        );
    }
}
