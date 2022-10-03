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
        io.wrapln(liso!(bold, "Welcome to Fight!"));
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
        self.io.status(Some(liso![
            inverse,
            " You: ",
            fg = if self.uhp <= MONSTER_ATTACK_DAMAGE {
                Some(Color::Red)
            }
            else if self.uhp <= (PLAYER_MAX_HP - PLAYER_POTION_HEAL) {
                Some(Color::Yellow)
            }
            else {
                Some(Color::Green)
            },
            format!("{:2}/{:2} HP", self.uhp, PLAYER_MAX_HP),
            fg = None,
            "  ",
            fg = if self.upot == 0 { Some(Color::Red) } else { None },
            format!("{:2}/{:2} potions", self.upot, PLAYER_START_POTIONS),
            fg = None,
            format!("            Enemy: {:3}/{:3} HP ",
                    self.mhp, MONSTER_MAX_HP),
        ]));
    }
    fn inner_loop(&mut self) {
        while self.uhp > 0 && self.mhp > 0 {
            self.update_status_line();
            self.io.prompt(liso![
                "What will you do?\n", fg = green, "> ", fg = None
            ], true, false);
            match self.io.blocking_read() {
                Response::Dead => panic!("Liso died!"),
                Response::Quit => return,
                Response::Input(wat) => {
                    self.io.wrapln(liso![fg=green,"> ",fg=none,&wat]);
                    if wat == "a" || wat == "attack" {
                        self.mhp -= PLAYER_ATTACK_DAMAGE;
                        self.io.wrapln(liso![
                            "You attack, dealing ", bold,
                            format!("{}", PLAYER_ATTACK_DAMAGE),
                            plain, " damage."
                        ]);
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
                                self.io.wrapln(liso![
                                    "You drink one of your potions, ",
                                    bold, fg = red,
                                    "wasting the whole thing!"
                                ]);
                            }
                            else {
                                self.io.wrapln(liso![
                                    "You drink one of your potions, healing \
                                     away ",
                                    bold,
                                    fg = if amount_healed < PLAYER_POTION_HEAL{
                                        Some(Color::Yellow)
                                    } else { None },
                                    format!("{}", amount_healed),
                                    reset,
                                    " damage.",
                                ]);
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
        self.io.wrapln(liso![
            "The enemy attacks, dealing ",
            bold,
            format!("{}", MONSTER_ATTACK_DAMAGE),
            plain,
            " damage."
        ]);
    }
}

fn main() {
    Fight::play();
    // We can use println here. If we've reached the end of `Fight::play()`,
    // then Liso has cleaned up after itself, and normal terminal output is
    // possible.
    println!("Bye bye!");
}
