use super::*;

pub enum Completion {
    InsertAtCursor { text: String },
    ReplaceWholeLine { new_line: String, new_cursor: usize },
}

/// Something that may know how to respond to a completion request, i.e. a tab
/// press.
pub trait Completor: Send {
    /// The user has pressed tab on this command line. The current state of
    /// the line, and the cursor position, are given. Return `None` if no
    /// completion is obvious in the given situation, or some completion
    /// otherwise.
    ///
    /// `output` is provided so that *you* can beep, print a message, display
    /// a notice, or any other combination thereof, explaining that, or why,
    /// completion is not possible, if you have to return `None`. That's up to
    /// you.
    ///
    /// `consecutive_presses` is the number of times that the user has hit the
    /// completion key, without hitting any other key in between. The first
    /// press will give `1`, the second will give `2`, and so forth. You may
    /// use this to provide more completion options, for example. You may also
    /// simply ignore it.
    fn complete(
        &mut self,
        output: &Output,
        input: &str,
        cursor: usize,
        consecutive_presses: NonZeroU32,
    ) -> Option<Completion>;
}
