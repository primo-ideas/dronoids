//! Built-in DASM programs. They are intentionally naive: the fun of the game
//! is to write better ones.

pub struct Builtin {
    pub name: &'static str,
    pub summary: &'static str,
    pub source: &'static str,
}

pub const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "miner",
        summary: "Goes to the nearest crystal, fills its hold, comes home.",
        source: r#"; MINER -- the starting program.
; Goes to the nearest crystal, digs it until its
; hold is full, then goes home to unload at the base.
; Flaw: rushes straight ahead and bumps into asteroids.

start:
    CARGO a
    JGE a, FULL, home
    DIST a, ORE          ; distance to the nearest crystal
    JEQ a, NONE, wander  ; nothing in sight?
    JLE a, 1, dig        ; within reach: dig
    SCAN b, ORE          ; b = angle to the crystal
    MOVE b
    JZ ok, dodge         ; blocked
    JMP start

dig:
    MINE
    JMP start

home:
    DIST a, BASE
    JLE a, 1, unload
    SCAN b, BASE
    MOVE b
    JZ ok, dodge
    JMP home

unload:
    DROP
    JMP start

dodge:                   ; a step aside, at random
    RAND c, 180
    ADD b, c
    SUB b, 90
    MOVE b
    JMP start

wander:
    RAND b, 360
    MOVE b
    MOVE b
    MOVE b
    JMP start
"#,
    },
    Builtin {
        name: "scout",
        summary: "Heads off in a straight line, turns when it runs into something.",
        source: r#"; SCOUT -- heads off in a straight line and changes
; course when it runs into something. Mines what it
; comes across, goes home when it is full. Useful to find
; faraway deposits and to widen the area one can build in.

    RAND d, 360          ; d = course
loop:
    CARGO a
    JGE a, FULL, home
    DIST a, ORE
    JEQ a, NONE, ahead
    JLE a, 1, dig
ahead:
    MOVE d
    JNZ ok, loop
    RAND d, 360          ; blocked: a new course
    JMP loop

dig:
    MINE
    JMP loop

home:
    DIST a, BASE
    JLE a, 1, unload
    SCAN b, BASE
    MOVE b
    JNZ ok, home
    RAND c, 180
    ADD b, c
    SUB b, 90
    MOVE b
    JMP home

unload:
    DROP
    RAND d, 360
    JMP loop
"#,
    },
    Builtin {
        name: "hunter",
        summary: "Hunts enemies down, or else heads for the base's target.",
        source: r#"; HUNTER -- hunts down enemy dronoids within range,
; or else attacks the nearest enemy base
; (spotted up to 40 units away), or else heads for
; its base's target: memory 0 = X, 1 = Y, 2 = 1
; when a target is set. Otherwise, patrols.

hunt:
    DIST a, ENEMY
    JEQ a, NONE, siege
    JLE a, 2, fire       ; within firing range
    SCAN b, ENEMY
    MOVE b
    JZ ok, dodge
    JMP hunt

siege:
    DIST a, EBASE
    JEQ a, NONE, target
    JLE a, 2, fire
    SCAN b, EBASE
    MOVE b
    JZ ok, dodge
    JMP hunt

fire:
    ATTACK
    JMP hunt

target:
    PEEK a, 2
    JZ a, patrol         ; no target set
    POSX a
    PEEK b, 0
    SUB b, a             ; b = east-west gap
    POSY a
    PEEK c, 1
    SUB c, a             ; c = north-south gap
    ANGLE b, b, c
    JEQ b, NONE, patrol
    MOVE b
    JZ ok, dodge
    JMP hunt

patrol:
    JNZ d, course        ; d = course (0 until one is picked)
    RAND d, 359
    INC d
course:
    MOVE d
    JNZ ok, hunt
    RAND d, 359
    INC d
    JMP hunt

dodge:
    RAND c, 180
    ADD b, c
    SUB b, 90
    MOVE b
    JMP hunt
"#,
    },
    Builtin {
        name: "guard",
        summary: "Stays near the base and fires at intruders.",
        source: r#"; GUARD -- stays near its base and
; fires at any enemy that comes close.

guard:
    DIST a, ENEMY
    JEQ a, NONE, back
    JLE a, 2, fire
    JGT a, 8, back       ; too far: never leave the base
    SCAN b, ENEMY
    MOVE b
    JMP guard

fire:
    ATTACK
    JMP guard

back:
    DIST a, BASE
    JLE a, 3, watch
    SCAN b, BASE
    MOVE b
    JNZ ok, guard
    RAND c, 180
    ADD b, c
    SUB b, 90
    MOVE b
    JMP guard

watch:
    WAIT
    JMP guard
"#,
    },
    Builtin {
        name: "prospector",
        summary: "Looks for relics and mines them: each shard is a step towards a perk for the base.",
        source: r#"; PROSPECTOR -- looks for relics. Heads off in a
; straight line; with a relic in sight, goes to it and
; mines it: every shard goes to the base's perks.
; Brings no crystal home.
; Flaw: wanders at random, and comes back to a relic
; its base has no more use for.

    RAND d, 360          ; d = course
loop:
    DIST a, RELIC
    JEQ a, NONE, ahead   ; no relic in sight
    JLE a, 1, dig        ; within reach
    SCAN b, RELIC
    MOVE b
    JNZ ok, loop
    JMP turn             ; blocked on the way
ahead:
    MOVE d
    JNZ ok, loop
turn:
    RAND d, 360          ; a new course
    MOVE d
    JMP loop

dig:
    MINE
    JNZ ok, loop
    RAND d, 360          ; nothing more to take here: move on
    MOVE d
    MOVE d
    MOVE d
    MOVE d
    JMP ahead
"#,
    },
];

/// Template for a brand-new program.
pub const TEMPLATE: &str = r#"; New program.
; Registers: a b c d (and ok, set to 1/0 by the last action)
; Angles in degrees: N=0 E=90 S=180 W=270
; Things: ORE RELIC ALLY ENEMY BASE EBASE WALL EMPTY
; F1: full help

start:
    SCAN a, ORE
    MOVE a
    JMP start
"#;

pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

/// The built-ins as they were named before the game was translated into
/// English, with the FNV-1a hash of each wording they shipped with, so that
/// the copies kept by players and saved sectors can be recognized.
const FORMER: &[(&str, &str, &[u64])] = &[
    ("mineur", "miner", &[0x400b_508f_08eb_cf55]),
    ("eclaireur", "scout", &[0x6720_70b2_c4ee_9198]),
    ("chasseur", "hunter", &[0xc92a_82f0_6bdb_5d46, 0xcd47_4320_75da_dfd5]),
    ("gardien", "guard", &[0x7840_7ede_869e_ca73]),
];

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// A program kept from before the translation, as it should be now: a
/// former built-in's name becomes the new one, and an untouched copy of a
/// former built-in becomes that built-in, whose instructions are the same
/// line for line. `None` when nothing changes.
pub fn modernized(name: &str, source: &str) -> Option<(String, String)> {
    let hash = fnv1a(source.trim());
    let renamed = FORMER.iter().find(|f| f.0 == name).map(|f| f.1);
    let copy_of = FORMER.iter().find(|f| f.2.contains(&hash)).map(|f| f.1);
    if renamed.is_none() && copy_of.is_none() {
        return None;
    }
    let source = copy_of.and_then(builtin).map_or(source, |b| b.source);
    Some((renamed.unwrap_or(name).to_string(), source.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::assemble;

    #[test]
    fn fnv1a_is_the_usual_one() {
        assert_eq!(fnv1a("abc"), 0xe71f_a219_0541_574b);
    }

    #[test]
    fn programs_of_other_names_are_left_alone() {
        assert_eq!(modernized("miner", BUILTINS[0].source), None);
        assert_eq!(modernized("mine", "; mine\nMINE\n"), None);
    }

    #[test]
    fn a_former_name_is_renamed_and_keeps_its_own_code() {
        let own = "; my own\nMINE\n";
        assert_eq!(modernized("chasseur", own), Some(("hunter".into(), own.into())));
    }

    #[test]
    fn every_former_built_in_has_its_new_one() {
        for (old, new, _) in FORMER {
            assert!(builtin(new).is_some(), "{old} -> {new}");
        }
    }

    #[test]
    fn builtins_assemble() {
        for b in BUILTINS {
            if let Err(e) = assemble(b.name, b.source) {
                panic!("{}: {:?}", b.name, e);
            }
        }
        assemble("t", TEMPLATE).unwrap();
    }
}
