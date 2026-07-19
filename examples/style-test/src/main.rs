use liso::{Color, InputOutput, Line, Response, Style, liso};

use clap::Parser;

#[derive(Parser)]
struct Invocation {
    /// Wait for enter to be pressed after each style is tested
    #[clap(short, long)]
    wait: bool,
}

const COLORS: &[(&str, Option<Color>)] = &[
    ("default", None),
    ("black", Some(Color::Black)),
    ("red", Some(Color::Red)),
    ("green", Some(Color::Green)),
    ("yellow", Some(Color::Yellow)),
    ("blue", Some(Color::Blue)),
    ("magenta", Some(Color::Magenta)),
    ("cyan", Some(Color::Cyan)),
    ("white", Some(Color::White)),
];

const STYLES: &[(&str, Style)] = &[
    ("bold", Style::BOLD),
    ("dim", Style::DIM),
    ("underline", Style::UNDERLINE),
    ("inverse", Style::INVERSE),
    ("italic", Style::ITALIC),
];

fn main() {
    let invocation = Invocation::parse();
    let mut io = InputOutput::new();
    for n in 0..2u32.pow(STYLES.len() as u32) {
        let mut style_list = Vec::with_capacity(STYLES.len());
        let mut style = Style::PLAIN;
        for (i, (name, value)) in STYLES.iter().enumerate() {
            if (n & (1 << i)) != 0 {
                style_list.push(*name);
                style |= *value;
            }
        }
        if style_list.is_empty() {
            style_list.push("plain");
        }
        let mut line = Line::new();
        line.set_style(style);
        line.add_text(style_list.join("+"));
        io.println(line);
        for (bg_name, color) in COLORS.iter() {
            let mut line = Line::new();
            line.set_style(style);
            line.set_bg_color(*color);
            for (_fg_name, color) in COLORS.iter() {
                line.set_fg_color(*color);
                line.add_text("X");
            }
            line.set_fg_color(None);
            line.add_text(" bg=");
            line.add_text(*bg_name);
            io.println(line);
        }
        if invocation.wait {
            io.prompt("Press enter to continue.", true, true);
            loop {
                match io.read_blocking() {
                    Response::Dead => return,
                    Response::Quit => {
                        io.println(liso!(dim, "^C"));
                        return;
                    }
                    Response::Input(_) | Response::Discarded(_) => {
                        break;
                    }
                    _ => (),
                }
            }
            io.prompt("", false, true);
        }
    }
}
