Liso (LEE-soh) is an acronym for Line Input with Simultaneous Output. It is a library for a particular kind of text-based Rust application; one where the user is expected to give command input at a prompt, but output can occur at any time. It provides simple line editing, and prevents input from clashing with output. It can be used asynchronously (with `tokio`) or synchronously (without).

It should work anywhere [Crossterm](https://crates.io/crates/crossterm) does:

- Windows 7 or later
- On UNIX, any system with an ANSI-compatible terminal (via crossterm) or a
  VT52-compatible terminal (via custom support).

See [the crate documentation](https://docs.rs/liso/latest/liso/) for more information.

**NOTE: WORK IN PROGRESS!** Not release ready!

# Line Editing Bindings

Liso provides line editing based on a commonly-used subset of the default GNU Readline bindings:

- **Control-B/F or Left/Right**: Move cursor.
- **Control-A or Home**: Go to beginning of line.
- **Control-E or End**: Go to end of line.
- **Control-W**: Delete leftward from cursor until reaching a `White_Space` character. ("Delete word")
- **Control-U**: Delete the whole input.
- **Control-K**: Delete everything to the right of the cursor.
- **Control-L**: Clear the display.
- **Control-C**: Send `Quit`.
- **Control-D**: Clear the input if there is any, or send `Finish` otherwise.
- **Control-T**: Send `Info`.
- **Control-\\ or Break**: Send `Break`.
- **Escape**: Send `Escape`.
- **Control-X**: Send `Swap`.
- **Return (control-M) or Enter (control-J)**: Send the current line of input.
- **Control-Z**: (UNIX only) Gracefully suspend ourselves, awaiting resumption by our parent shell.

More bindings may be added in the future, and some of these are subject to change before 1.0.

# Future

## Release blockers

Features that are currently set as prerequisites for a 1.0 release.

- Control-G on input
- History
- Tab completion
- Windows testing
- Document the VT52 support
- Move all the channels into TtyState
- Optimize, hintify, reversiblize, etc. `LineCharIterator`
- Clear all TODOs

## TODO, eventually

Features that are still planned, but won't block 1.0.

- Control-V on input

## Deferred features

Features that are desirable, but proved too difficult to implement.

- Control-S/-Q on input
- Squelch output feature (with mandatory status line, related to above)

## Pie in the sky

Features that I'd like, but that I am unlikely ever to have the time to implement:

- Right-to-left text support
- Better combining character support

# Legalese

Liso is copyright 2022, Solra Bizna, and licensed under either of:

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or
   <http://www.apache.org/licenses/LICENSE-2.0>)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the Liso crate by you, as defined
in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
