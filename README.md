Liso (LEE-soh) is an acronym for Line Input with Simultaneous Output. It is a library for writing line-oriented programs: programs that take input in the form of lines, and produce output in the form of lines.

Main features:

- Line editing à la Readline
- Customizable prompt
- Output displayed separately from input
- Simultaneous output from unlimited threads / tasks
- Status line, displayed above input and below output
- Pipeline-savvy (interactivity features are automatically disabled when used in a pipeline)
- Optional async support (with `tokio`)

Supported platforms:

- Windows 7 or later (completely untested)
- Any OS with an ANSI-compliant terminal (if you don't know if your terminal is ANSI-compliant, it is)
- Any OS with a [VT52](#vt52-support)-compatible terminal

See [the crate documentation](https://docs.rs/liso/latest/liso/) for more information, or [the examples](https://github.com/SolraBizna/liso/tree/main/examples) for complete example programs.

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

These bindings are subject to change. More bindings may be added in the future, the default bindings may change, and user-specified bindings may one day be possible.

# VT52 support!?

The Atari ST personal computer, released in 1985, came with a VT52 emulator in its onboard ROM. While the ability to serve as a cheap remote terminal was warmly welcomed in the market, the VT52 was a strange choice of terminals to emulate, since, even back in 1985, it was already considered woefully obsolete. Nevertheless, this emulator served as as testbed for support for strange, non-ANSI, non-Crossterm terminals in Liso.

If the `TERM` environment variable exists, and the base type (to the left of the `-`, if any) is `st52`, `tw52`, `tt52`, `at`, `atari`, `atarist`, `atari_st`, `vt52`, `stv52`, or `stv52pc`, then Liso's VT52 support will be activated. It will try to figure out the number of colors and special feature support based on which particular terminal type you've selected and how big it is. You should use one of the following values:

- `TERM=st52-m`, 80 x 50: Atari ST with monochrome monitor (high res).
- `TERM=st52`, 80 x 25: Atari ST with color monitor (medium res, 4 colors).
- `TERM=st52`, 40 x 25: Atari ST with color monitor (low res, 16 colors).
- `TERM=atari`, any size: Later Atari with color monitor (assumes 16 colors).
- `TERM=vt52`, any size: Real VT52 (untested).

Input and output work. Special characters other than control keys don't work, I will need to do more testing to understand why. Testing Liso against Atari's VT52 emulator was extremely helpful in optimizing the redrawing routine, and teasing out some edge cases in the style handling.

# Help Wanted

I don't have a Windows machine in any real sense, so I can't test whether this crate functions on Windows. It *should*, since it uses Crossterm, but I would appreciate reports from Windows users and/or developers.

I have no idea how well Liso works for visually-impaired users. If you use command line applications with a screen reader or a Braille terminal, I would greatly appreciate it if you got in touch with me. I would love to learn more about how I can improve your experience with Liso-based programs.

# Future

Tab completion and history support are on the roadmap. Perhaps, someday, RTL / bidirectional support as well.

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
