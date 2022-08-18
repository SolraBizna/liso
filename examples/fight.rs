use std::{
    time::Duration,
};

use liso::*;

const PLAYER_MAX_HP: i32 = 50;
const PLAYER_START_POTIONS: i32 = 3;
const PLAYER_POTION_HEAL: i32 = 30;
const PLAYER_ATTACK_DAMAGE: i32 = 10;
const MONSTER_MAX_HP: i32 = 100;
const MONSTER_ATTACK_DAMAGE: i32 = 10;

struct Fight {
    io: IO,
    uhp: i32,
    upot: i32,
    mhp: i32,
}

impl Fight {
    fn play() {
        let io = IO::new();
        let mut line = Line::new();
        line.set_style(Style::BOLD).add_text("Welcome to Fight!");
        io.wrapln(line);
        io.wrapln("Your goal in life is to defeat this evil monster, before \
                      they can defeat you!");
        let mut fight = Fight {
            io,
            uhp: PLAYER_MAX_HP,
            upot: PLAYER_START_POTIONS,
            mhp: MONSTER_MAX_HP,
        };
        fight.inner_loop();
    }
    fn update_status_line(&mut self) {
        let mut status_line = Line::new();
        status_line.set_style(Style::INVERSE);
        status_line.add_text(" You: ");
        if self.uhp <= MONSTER_ATTACK_DAMAGE {
            status_line.set_fg_color(Some(Color::Red));
        }
        else if self.uhp <= (PLAYER_MAX_HP - PLAYER_POTION_HEAL) {
            status_line.set_fg_color(Some(Color::Yellow));
        }
        else {
            status_line.set_fg_color(Some(Color::Green));
        }
        status_line.add_text(format!("{:2}/{:2} HP",
                                     self.uhp, PLAYER_MAX_HP));
        status_line.set_fg_color(None);
        status_line.add_text("  ");
        if self.upot == 0 {
            status_line.set_fg_color(Some(Color::Red));
        }
        status_line.add_text(format!("{:2}/{:2} potions",
                                     self.upot,
                                     PLAYER_START_POTIONS));
        status_line.set_fg_color(None);
        status_line.add_text(format!("            Enemy: {:3}/{:3} HP ",
                                     self.mhp, MONSTER_MAX_HP));
        self.io.status(Some(status_line));
    }
    fn inner_loop(&mut self) {
        while self.uhp > 0 && self.mhp > 0 {
            self.update_status_line();
            let mut line = Line::new();
            line.add_text("What will you do?\n");
            line.set_fg_color(Some(Color::Green));
            line.add_text("> ");
            line.set_fg_color(None);
            self.io.prompt(line, true, false);
            match self.io.blocking_read() {
                Response::Dead => panic!("LISO died!"),
                Response::Quit => return,
                Response::Input(wat) => {
                    let mut line = Line::new();
                    line.set_fg_color(Some(Color::Green));
                    line.add_text("> ");
                    line.set_fg_color(None);
                    line.add_text(&wat);
                    self.io.wrapln(line);
                    if wat == "a" || wat == "attack" {
                        self.mhp -= PLAYER_ATTACK_DAMAGE;
                        let mut line = Line::new();
                        line.add_text("You attack, dealing ");
                        line.set_style(Style::BOLD);
                        line.add_text(format!("{}", PLAYER_ATTACK_DAMAGE));
                        line.clear_style();
                        line.add_text(" damage.");
                        self.io.wrapln(line);
                        self.mon_attack();
                    }
                    else if wat == "p" || wat == "potion" {
                        if self.upot == 0 {
                            self.io.wrapln("You are out of potions.");
                        }
                        else {
                            self.upot -= 1;
                            let new_hp = (self.uhp + PLAYER_POTION_HEAL).min(PLAYER_MAX_HP);
                            let amount_healed = new_hp.saturating_sub(self.uhp);
                            if amount_healed == 0 {
                                let mut line = Line::new();
                                line.add_text("You drink one of your potions, ");
                                line.set_style(Style::BOLD);
                                line.set_fg_color(Some(Color::Red));
                                line.add_text("wasting the whole thing!");
                                self.io.wrapln(line);
                            }
                            else {
                                let mut line = Line::new();
                                line.add_text("You drink one of your potions, healing away ");
                                line.set_style(Style::BOLD);
                                if amount_healed < PLAYER_POTION_HEAL {
                                    line.set_fg_color(Some(Color::Yellow));
                                }
                                line.add_text(format!("{}", amount_healed));
                                line.clear_style();
                                line.set_fg_color(None);
                                line.add_text(" damage.");
                                self.io.wrapln(line);
                                if amount_healed < PLAYER_POTION_HEAL {
                                    self.io.wrapln("Some of that potion was wasted!");
                                }
                                self.uhp = new_hp;
                            }
                            self.mon_attack();
                        }
                    }
                    else {
                        self.io.wrapln("Your choices are 'attack' or \
                                          'potion'.");
                    }
                },
                other => {
                    self.io.notice(format!("unknown key {}",
                                           other.as_unknown() as char),
                                   Duration::from_secs(1));
                },
            }
        }
        if self.uhp <= 0 {
            self.io.wrapln("You lose!");
        }
        else if self.mhp <= 0 {
            self.io.wrapln("You win!");
        }
    }
    fn mon_attack(&mut self) {
        self.uhp -= MONSTER_ATTACK_DAMAGE;
        let mut line = Line::new();
        line.add_text("The enemy attacks, dealing ");
        line.set_style(Style::BOLD);
        line.add_text(format!("{}", MONSTER_ATTACK_DAMAGE));
        line.clear_style();
        line.add_text(" damage.");
        self.io.wrapln(line);
    }
}

fn main() {
    Fight::play();
    // We can use println here. If we've reached the end of `Fight::play()`,
    // then Liso has cleaned up after itself, and normal terminal output is
    // possible.
    println!("Bye bye!");
}
