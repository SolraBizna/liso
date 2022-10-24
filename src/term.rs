use super::*;

use std::{
    io::{Stdout, Read},
};

mod cross;
use cross::Crossterminal;
mod vt52;
use vt52::Vt52;

/// A wrapper for a particular terminal engine, supporting input and output.
///
/// We always use `crossterm` for enabling/disabling raw mode and for detecting
/// the size of the terminal, but if the TERM/TERMINFO environment variables
/// are set appropriately, we might use one of the other variants.

pub(crate) trait Term {
    /// Set the current attributes. (If possible, defer the actual control
    /// code outputting until the next time one of the `print_*` functions is
    /// called.)
    fn set_attrs(&mut self, style: Style,
                 fg: Option<Color>, bg: Option<Color>) -> LifeOrDeath;
    fn reset_attrs(&mut self) -> LifeOrDeath;
    fn print(&mut self, text: &str) -> LifeOrDeath;
    fn print_char(&mut self, char: char) -> LifeOrDeath;
    fn print_spaces(&mut self, spaces: usize) -> LifeOrDeath;
    fn move_cursor_up(&mut self, amt: u32) -> LifeOrDeath;
    fn move_cursor_down(&mut self, amt: u32) -> LifeOrDeath;
    fn move_cursor_left(&mut self, amt: u32) -> LifeOrDeath;
    fn move_cursor_right(&mut self, amt: u32) -> LifeOrDeath;
    fn newline(&mut self) -> LifeOrDeath;
    fn carriage_return(&mut self) -> LifeOrDeath;
    fn bell(&mut self) -> LifeOrDeath;
    fn clear_all_and_reset(&mut self) -> LifeOrDeath;
    fn clear_forward_and_reset(&mut self) -> LifeOrDeath;
    fn clear_to_end_of_line(&mut self) -> LifeOrDeath;
    fn hide_cursor(&mut self) -> LifeOrDeath;
    fn show_cursor(&mut self) -> LifeOrDeath;
    fn get_width(&mut self) -> u32;
    fn cur_style(&self) -> Style;
    fn flush(&mut self) -> LifeOrDeath;
    fn suspend(&mut self) -> LifeOrDeath;
    fn unsuspend(&mut self) -> LifeOrDeath;
    fn cleanup(&mut self) -> LifeOrDeath;
}

pub(crate) fn new_term(req_tx: &std_mpsc::Sender<Request>)
-> Result<Box<dyn Term>, DummyError> {
    if let Ok(term) = std::env::var("TERM") {
        let main = term.split("-").next().unwrap_or("");
        match main {
            "st52" | "tw52" | "tt52" | "at" | "atari" | "atarist" | "atari_st"
                | "vt52" | "stv52" | "stv52pc" => {
                    // A real VT52, or (way more likely) an Atari ST (or
                    // descendant) emulating one
                    let monochrome = main == "vt52" || term.ends_with("-m");
                    let num_colors = if monochrome { 2 }
                    else if main.contains("st") {
                        // When it's an Atari ST, there are three possibilities
                        // - 80 x 50: high res = monochrome
                        // - 80 x 25: medium res = 4 colors
                        // - 40 x 25: low res = 16 colors
                        // Anything else is a misconfiguration, so we just
                        // assume monochrome to be safe.
                        match crossterm::terminal::size().unwrap_or((80,25)) {
                            (80, 50) => 2,
                            (80, 25) => 4,
                            (40, 25) => 16,
                            _ => {
                                eprintln!("Your terminal is configured \
                                           incorrectly. Assuming monochrome.");
                                2
                            },
                        }
                    }
                    else { 16 };
                    return Ok(Box::new(Vt52::new(req_tx.clone(), num_colors)?))
                },
            _ => (), // fall through
        }
    }
    Ok(Box::new(Crossterminal::new(req_tx.clone())?))
}
