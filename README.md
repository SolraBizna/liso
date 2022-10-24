Liso (LEE-soh) is an acronym for Line Input with Simultaneous Output. It is a library for a particular kind of text-based Rust application; one where the user is expected to give command input at a prompt, but output can occur at any time. It provides simple line editing, and prevents input from clashing with output. It can be used asynchronously (with `tokio`) or synchronously (without).

It should work anywhere [Crossterm](https://crates.io/crates/crossterm) does:

- Windows 7 or later
- On UNIX, any system with an ANSI-compatible terminal (via crossterm) or a
  VT52-compatible terminal (via custom support).

See [the crate documentation](https://docs.rs/liso/latest/liso/) for more information.

**NOTE: WORK IN PROGRESS!** Not release ready!

# Line Editing Bindings

Liso provides line editing based on a commonly-used subset of the default GNU Readline bindings:

- **Escape**: Send `Escape`.
- **Return (control-M) or Enter (control-J)**: Send the current line of input.
- **Control-A or Home**: Go to beginning of line.
- **Control-B/F or Left/Right**: Move cursor.
- **Control-C**: Send `Quit`.
- **Control-D**: Discard the input if there is any, or send `Finish` otherwise.
- **Control-E or End**: Go to end of line.
- **Control-G**: Discard the input if there is any, leaving feedback of the aborted entry.
- **Control-K**: Cut (**k**ill) everything after the cursor.
- **Control-L**: Clear the display.
- **Control-T**: Send `Info`.
- **Control-U**: Cut (kill) everything before the cursor.
- **Control-W**: Delete leftward from cursor until reaching a `White_Space` character. ("Delete **w**ord")
- **Control-X**: Send `Swap`.
- **Control-Y**: Paste (**y**ank) the last text that was cut.
- **Control-Z**: (UNIX only) Gracefully suspend ourselves, awaiting resumption by our parent shell.
- **Control-\\ or Break**: Send `Break`.

More bindings may be added in the future, and some of these are subject to change before 1.0.

# Future

## Release blockers

Features that are currently set as prerequisites for a 1.0 release.

- History
- Tab completion
- Windows testing
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

# VT52 support!?

The Atari ST personal computer, released in 1985, came with a VT52 emulator in its onboard ROM. While the ability to serve as a cheap remote terminal was warmly welcomed in the market, the VT52 was a strange choice of terminals to emulate, since, even back in 1985, it was already considered woefully obsolete. Nevertheless, this emulator served as as testbed for support for strange, non-ANSI, non-Crossterm terminals in Liso.

If the `TERM` environment variable exists, and the base type (to the left of the `-`, if any) is `st52`, `tw52`, `tt52`, `at`, `atari`, `atarist`, `atari_st`, `vt52`, `stv52`, or `stv52pc`, then Liso's VT52 support will be activated. It will try to figure out the number of colors and special feature support based on which particular terminal type you've selected and how wide it is. You should use one of the following values:

- `TERM=st52-m`, 80 x 50: Atari ST with monochrome monitor (high res).
- `TERM=st52`, 80 x 25: Atari ST with color monitor (medium res, 4 colors).
- `TERM=st52`, 40 x 25: Atari ST with color monitor (low res, 16 colors).
- `TERM=atari`, any size: Later Atari with color monitor (assumes 16 colors).
- `TERM=vt52`, any size: Real VT52 (untested).

Input and output work. Special characters other than control keys don't work, I will need to do more testing to understand why. Testing Liso against Atari's VT52 emulator was extremely helpful in optimizing the redrawing routine, and teasing out some edge cases in the style handling.

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
