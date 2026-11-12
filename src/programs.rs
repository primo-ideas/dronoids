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
; Goes to the nearest crystal, mines until the hold
; is full, then unloads it at the base.
; Flaw: rushes straight at things, and only steps
; aside at random when something is in the way.

start:
    IFFULL home
    IFNEAR ORE, dig
    GO ORE
    IFFAIL wander        ; nothing in sight, or blocked
    JUMP start
dig:
    MINE
    JUMP start
home:
    IFNEAR BASE, unload
    GO BASE
    IFFAIL wander
    JUMP home
unload:
    DROP
    JUMP start
wander:
    EXPLORE
    JUMP start
"#,
    },
    Builtin {
        name: "scout",
        summary: "Heads off in a straight line, turns when it runs into something.",
        source: r#"; SCOUT -- heads off in a straight line and changes
; course when it runs into something. Mines what it
; comes across, goes home when it is full. Useful to
; find faraway deposits and relics.

start:
    IFFULL home
    IFNEAR ORE, dig
    EXPLORE
    JUMP start
dig:
    MINE
    JUMP start
home:
    IFNEAR BASE, unload
    GO BASE
    IFFAIL dodge
    JUMP home
unload:
    DROP
    JUMP start
dodge:
    EXPLORE
    JUMP home
"#,
    },
    Builtin {
        name: "hunter",
        summary: "Hunts enemies down, or else heads for the base's target.",
        source: r#"; HUNTER -- hunts down enemy dronoids in sight,
; or else attacks an enemy base (spotted up to 40
; units away), or else heads for its base's target:
; memory 0 = X, 1 = Y, 2 = 1 when a target is set.
; Otherwise, patrols.

hunt:
    IFNEAR ENEMY, fire
    IFSEE ENEMY, chase
    IFNEAR EBASE, fire
    IFSEE EBASE, siege
    PEEK a, 2
    JZ a, patrol         ; no target set
    POSX a
    PEEK b, 0
    SUB b, a             ; b = east-west gap
    POSY a
    PEEK c, 1
    SUB c, a             ; c = north-south gap
    ANGLE b, b, c
    MOVE b
    IFFAIL patrol
    JUMP hunt
chase:
    GO ENEMY
    IFFAIL patrol
    JUMP hunt
siege:
    GO EBASE
    IFFAIL patrol
    JUMP hunt
fire:
    ATTACK
    JUMP hunt
patrol:
    EXPLORE
    JUMP hunt
"#,
    },
    Builtin {
        name: "guard",
        summary: "Stays near the base and fires at intruders.",
        source: r#"; GUARD -- stays near its base and fires at any
; enemy that comes close.

guard:
    IFNEAR ENEMY, fire
    DIST a, BASE
    JGT a, 6, back       ; never strays far from the base
    IFSEE ENEMY, chase
    WAIT
    JUMP guard
chase:
    GO ENEMY
    JUMP guard
fire:
    ATTACK
    JUMP guard
back:
    GO BASE
    IFFAIL dodge
    JUMP guard
dodge:
    EXPLORE
    JUMP guard
"#,
    },
    Builtin {
        name: "prospector",
        summary: "Looks for relics and mines them: each shard is a step towards a perk for the base.",
        source: r#"; PROSPECTOR -- looks for relics. Heads off in a
; straight line; with a relic in sight, goes to it and
; mines it: every shard goes to the base's perks.
; Brings no crystal home.
; Flaw: comes back to a relic its base has no more
; use for.

start:
    IFNEAR RELIC, dig
    IFSEE RELIC, go
    EXPLORE
    JUMP start
go:
    GO RELIC
    IFFAIL away
    JUMP start
dig:
    MINE
    IFFAIL away          ; nothing more to take here
    JUMP start
away:
    EXPLORE
    EXPLORE
    EXPLORE
    JUMP start
"#,
    },
];

/// Template for a brand-new program.
pub const TEMPLATE: &str = r#"; New program: drag instructions in from the
; right, or type them. F1: full help.

start:
    GO ORE
    IFFAIL wander
    JUMP start
wander:
    EXPLORE
    JUMP start
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

/// Earlier English wordings of the built-ins (the FNV-1a hash of each),
/// before they were written with the simple instructions. Unlike the
/// translation, the instructions changed: only the player's library takes
/// the new wording, never a program dronoids are running.
const SUPERSEDED: &[(&str, &[u64])] = &[
    ("miner", &[0x9e43_3922_2fcc_af46, 0xfe26_3ca9_689a_cc25]),
    ("scout", &[0xde39_76f3_278f_8f21]),
    ("hunter", &[0x07e7_872f_7f3c_53b4, 0x9dd5_a20f_ec23_0aba]),
    ("guard", &[0x2dfb_50d6_acfd_6b4a, 0x4a35_d1a3_0024_29cd]),
    ("prospector", &[0x8f2a_e80b_9530_ad1d, 0xbe30_7236_dc13_a136]),
];

/// The current wording of a built-in, if `source` is an untouched copy of
/// one of its earlier English wordings.
pub fn superseded(name: &str, source: &str) -> Option<&'static str> {
    let hash = fnv1a(source.trim());
    SUPERSEDED.iter().find(|s| s.0 == name && s.1.contains(&hash)).and_then(|s| builtin(s.0)).map(|b| b.source)
}

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
    fn untouched_copies_of_former_wordings_take_the_simple_one() {
        // The miner as it was worded until the simple instructions came.
        let before = "; MINER -- the starting program.
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
";
        assert_eq!(superseded("miner", before), Some(BUILTINS[0].source));
        assert_eq!(superseded("miner", &format!("; mine
{before}")), None, "a copy of one's own stays");
        assert_eq!(superseded("scout", before), None);
        for b in BUILTINS {
            assert_eq!(superseded(b.name, b.source), None, "{}: the current wording is not superseded", b.name);
        }
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
