//! DASM — the Dronoid ASseMbly language.
//!
//! A deliberately small, playful assembly (in the spirit of Human Resource
//! Machine). Each dronoid owns four registers `a b c d`, a read-only `ok` flag
//! set by the last action, a tiny call stack and a private RNG. Each
//! factory owns 16 cells of "hive memory" shared by the dronoids it spawned.
//!
//! Directions are compass angles in degrees: 0 = north, 90 = east...
//!
//! When idle, a dronoid runs instructions until it reaches an *action*
//! (MOVE, MINE, DROP, ATTACK, WAIT), which takes some time to complete.
//! Sensors and arithmetic are instantaneous, up to `MAX_OPS_PER_TICK`
//! instructions; past that the dronoid stalls for a moment.

use serde::{Deserialize, Serialize};

use crate::geo;
use crate::rng::splitmix;
use crate::rules::{CARGO_CAP, HIVE_MEM, MAX_OPS_PER_TICK, STACK_DEPTH};

// ---------------------------------------------------------------------------
// Values shared with the simulation
// ---------------------------------------------------------------------------

pub const DIR_N: i32 = 0;
pub const DIR_E: i32 = 90;
pub const DIR_S: i32 = 180;
pub const DIR_W: i32 = 270;
pub const NONE: i32 = -1;

/// What a tile contains, as seen by LOOK, and what SCAN / DIST... search for.
pub mod thing {
    pub const EMPTY: i32 = 0;
    pub const WALL: i32 = 1;
    pub const ORE: i32 = 2;
    pub const ALLY: i32 = 3;
    pub const ENEMY: i32 = 4;
    pub const BASE: i32 = 5;
    pub const EBASE: i32 = 6;
}

pub const CONSTANTS: &[(&str, i32)] = &[
    ("N", DIR_N),
    ("E", DIR_E),
    ("S", DIR_S),
    ("W", DIR_W),
    ("NONE", NONE),
    ("EMPTY", thing::EMPTY),
    ("WALL", thing::WALL),
    ("ORE", thing::ORE),
    ("ALLY", thing::ALLY),
    ("ENEMY", thing::ENEMY),
    ("BASE", thing::BASE),
    ("EBASE", thing::EBASE),
    ("FULL", CARGO_CAP),
];

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reg {
    A,
    B,
    C,
    D,
    Ok,
}

impl Reg {
    pub const ALL: [Reg; 4] = [Reg::A, Reg::B, Reg::C, Reg::D];
    pub fn name(self) -> &'static str {
        match self {
            Reg::A => "a",
            Reg::B => "b",
            Reg::C => "c",
            Reg::D => "d",
            Reg::Ok => "ok",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Val {
    R(Reg),
    K(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arith {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Rand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    Always,
    Z,
    Nz,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

impl Cond {
    fn eval(self, a: i32, b: i32) -> bool {
        match self {
            Cond::Always => true,
            Cond::Z => a == 0,
            Cond::Nz => a != 0,
            Cond::Eq => a == b,
            Cond::Ne => a != b,
            Cond::Lt => a < b,
            Cond::Gt => a > b,
            Cond::Le => a <= b,
            Cond::Ge => a >= b,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensor {
    Scan,
    Dist,
    Dx,
    Dy,
    Count,
    Look,
    Cargo,
    Hp,
    PosX,
    PosY,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Move,
    Mine,
    Drop,
    Attack,
    Wait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instr {
    Arith(Arith, Reg, Val),
    Inc(Reg),
    Dec(Reg),
    Jump(Cond, Val, Val, u16),
    Call(u16),
    Ret,
    Sense(Sensor, Reg, Val),
    Angle(Reg, Val, Val),
    Peek(Reg, Val),
    Poke(Val, Val),
    Act(Action, Val),
}

// ---------------------------------------------------------------------------
// Mnemonic table (drives the assembler, the syntax highlighter and the docs)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Data,
    Flow,
    Sense,
    Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arg {
    R,
    V,
    L,
}

pub struct Mnemonic {
    pub name: &'static str,
    args: &'static [Arg],
    pub cat: Cat,
    pub syntax: &'static str,
    pub help: &'static str,
}

use Arg::{L, R, V};

macro_rules! mn {
    ($name:literal, [$($a:ident),*], $cat:ident, $syn:literal, $help:literal) => {
        Mnemonic { name: $name, args: &[$($a),*], cat: Cat::$cat, syntax: $syn, help: $help }
    };
}

pub const MNEMONICS: &[Mnemonic] = &[
    mn!("SET", [R, V], Data, "SET r, v", "r = v"),
    mn!("ADD", [R, V], Data, "ADD r, v", "r = r + v"),
    mn!("SUB", [R, V], Data, "SUB r, v", "r = r - v"),
    mn!("MUL", [R, V], Data, "MUL r, v", "r = r * v"),
    mn!("DIV", [R, V], Data, "DIV r, v", "r = r / v (0 si v = 0)"),
    mn!("MOD", [R, V], Data, "MOD r, v", "r = r modulo v (toujours >= 0)"),
    mn!("RAND", [R, V], Data, "RAND r, v", "r = hasard entre 0 et v-1"),
    mn!("INC", [R], Data, "INC r", "r = r + 1"),
    mn!("DEC", [R], Data, "DEC r", "r = r - 1"),
    mn!("JMP", [L], Flow, "JMP lbl", "saute à l'étiquette"),
    mn!("JZ", [V, L], Flow, "JZ v, lbl", "saute si v = 0"),
    mn!("JNZ", [V, L], Flow, "JNZ v, lbl", "saute si v != 0"),
    mn!("JEQ", [V, V, L], Flow, "JEQ x, y, lbl", "saute si x = y"),
    mn!("JNE", [V, V, L], Flow, "JNE x, y, lbl", "saute si x != y"),
    mn!("JLT", [V, V, L], Flow, "JLT x, y, lbl", "saute si x < y"),
    mn!("JGT", [V, V, L], Flow, "JGT x, y, lbl", "saute si x > y"),
    mn!("JLE", [V, V, L], Flow, "JLE x, y, lbl", "saute si x <= y"),
    mn!("JGE", [V, V, L], Flow, "JGE x, y, lbl", "saute si x >= y"),
    mn!("CALL", [L], Flow, "CALL lbl", "appelle un sous-programme"),
    mn!("RET", [], Flow, "RET", "revient après le CALL"),
    mn!("ANGLE", [R, V, V], Data, "ANGLE r, dx, dy", "r = angle (degrés) du vecteur dx, dy"),
    mn!("SCAN", [R, V], Sense, "SCAN r, chose", "r = angle vers la chose la plus proche (NONE si rien)"),
    mn!("DIST", [R, V], Sense, "DIST r, chose", "r = distance jusqu'à la chose la plus proche (NONE si rien)"),
    mn!("DX", [R, V], Sense, "DX r, chose", "r = écart est-ouest vers la chose (+ = est)"),
    mn!("DY", [R, V], Sense, "DY r, chose", "r = écart nord-sud vers la chose (+ = sud)"),
    mn!("COUNT", [R, V], Sense, "COUNT r, chose", "r = nombre de choses à portée"),
    mn!("LOOK", [R, V], Sense, "LOOK r, angle", "r = ce qu'il y a juste devant, dans cette direction"),
    mn!("CARGO", [R], Sense, "CARGO r", "r = minerai transporté"),
    mn!("HP", [R], Sense, "HP r", "r = points de vie"),
    mn!("POSX", [R], Sense, "POSX r", "r = position est-ouest du dronoid"),
    mn!("POSY", [R], Sense, "POSY r", "r = position nord-sud du dronoid"),
    mn!("PEEK", [R, V], Sense, "PEEK r, i", "r = mémoire de ruche[i]"),
    mn!("POKE", [V, V], Sense, "POKE i, v", "mémoire de ruche[i] = v"),
    mn!("MOVE", [V], Action, "MOVE angle", "avance d'un pas dans cette direction"),
    mn!("MINE", [], Action, "MINE", "extrait 1 cristal du gisement à portée"),
    mn!("DROP", [], Action, "DROP", "décharge la soute dans une usine alliée à portée"),
    mn!("ATTACK", [], Action, "ATTACK", "tire sur l'ennemi le plus proche à portée"),
    mn!("WAIT", [], Action, "WAIT", "patiente un instant"),
];

pub fn mnemonic(name: &str) -> Option<&'static Mnemonic> {
    MNEMONICS.iter().find(|m| m.name.eq_ignore_ascii_case(name))
}

pub fn constant(name: &str) -> Option<i32> {
    CONSTANTS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| *v)
}

pub fn parse_reg(tok: &str) -> Option<Reg> {
    match tok.to_ascii_lowercase().as_str() {
        "a" => Some(Reg::A),
        "b" => Some(Reg::B),
        "c" => Some(Reg::C),
        "d" => Some(Reg::D),
        "ok" => Some(Reg::Ok),
        _ => None,
    }
}

fn parse_int(tok: &str) -> Option<i32> {
    let t = tok.strip_prefix('+').unwrap_or(tok);
    t.parse::<i32>().ok()
}

fn parse_val(tok: &str) -> Result<Val, String> {
    if let Some(r) = parse_reg(tok) {
        return Ok(Val::R(r));
    }
    if let Some(k) = parse_int(tok) {
        return Ok(Val::K(k));
    }
    if let Some(k) = constant(tok) {
        return Ok(Val::K(k));
    }
    Err(format!("valeur inconnue « {tok} »"))
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// Strip `;`, `#` and `//` comments.
pub fn strip_comment(line: &str) -> &str {
    let mut end = line.len();
    for pat in [";", "#", "//"] {
        if let Some(i) = line.find(pat) {
            end = end.min(i);
        }
    }
    &line[..end]
}

// ---------------------------------------------------------------------------
// Assembler
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsmError {
    /// 0-based source line.
    pub line: usize,
    pub msg: String,
}

#[derive(Debug)]
pub struct Program {
    pub name: String,
    pub source: String,
    pub code: Vec<Instr>,
    /// Source line (0-based) of each instruction.
    pub lines: Vec<u32>,
}

impl Program {
    pub fn empty(name: &str) -> Program {
        Program { name: name.to_string(), source: String::new(), code: vec![], lines: vec![] }
    }

    pub fn line_of(&self, pc: u16) -> Option<usize> {
        self.lines.get(pc as usize).map(|l| *l as usize)
    }
}

/// Labels and code size of a source, used by the editor.
pub fn labels(source: &str) -> Vec<String> {
    let mut out = vec![];
    for line in source.lines() {
        let mut rest = strip_comment(line).trim();
        while let Some(i) = rest.find(':') {
            let name = rest[..i].trim();
            if !is_ident(name) {
                break;
            }
            out.push(name.to_ascii_lowercase());
            rest = rest[i + 1..].trim();
        }
    }
    out
}

pub fn assemble(name: &str, source: &str) -> Result<Program, Vec<AsmError>> {
    struct Pending<'a> {
        line: usize,
        mn: &'static Mnemonic,
        args: Vec<&'a str>,
    }

    let mut errors = vec![];
    let mut labels: Vec<(String, u16, usize)> = vec![];
    let mut pending: Vec<Pending> = vec![];

    for (ln, raw) in source.lines().enumerate() {
        let mut rest = strip_comment(raw).trim();
        // Leading labels: `loop:` or `loop: MOVE N`.
        while let Some(i) = rest.find(':') {
            let label = rest[..i].trim();
            if !is_ident(label) {
                errors.push(AsmError { line: ln, msg: format!("étiquette invalide « {label} »") });
                rest = "";
                break;
            }
            let key = label.to_ascii_lowercase();
            if let Some((_, _, prev)) = labels.iter().find(|(n, _, _)| *n == key) {
                errors.push(AsmError {
                    line: ln,
                    msg: format!("étiquette « {label} » déjà définie ligne {}", prev + 1),
                });
            } else {
                labels.push((key, pending.len() as u16, ln));
            }
            rest = rest[i + 1..].trim();
        }
        if rest.is_empty() {
            continue;
        }
        let mut toks = rest.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty());
        let op = toks.next().unwrap_or_default();
        let args: Vec<&str> = toks.collect();
        match mnemonic(op) {
            None => errors.push(AsmError { line: ln, msg: format!("instruction inconnue « {op} »") }),
            Some(mn) => {
                if args.len() != mn.args.len() {
                    errors.push(AsmError {
                        line: ln,
                        msg: format!(
                            "{} attend {} opérande(s), {} donnée(s) — syntaxe : {}",
                            mn.name,
                            mn.args.len(),
                            args.len(),
                            mn.syntax
                        ),
                    });
                } else {
                    pending.push(Pending { line: ln, mn, args });
                }
            }
        }
    }

    if pending.len() > u16::MAX as usize {
        errors.push(AsmError { line: 0, msg: "programme trop long".into() });
    }

    let mut code = Vec::with_capacity(pending.len());
    let mut lines = Vec::with_capacity(pending.len());
    for p in &pending {
        match build(p.mn, &p.args, &labels) {
            Ok(ins) => {
                code.push(ins);
                lines.push(p.line as u32);
            }
            Err(msg) => errors.push(AsmError { line: p.line, msg }),
        }
    }

    if errors.is_empty() {
        Ok(Program { name: name.to_string(), source: source.to_string(), code, lines })
    } else {
        errors.sort_by_key(|e| e.line);
        Err(errors)
    }
}

fn build(mn: &Mnemonic, args: &[&str], labels: &[(String, u16, usize)]) -> Result<Instr, String> {
    let reg = |i: usize| -> Result<Reg, String> {
        match parse_reg(args[i]) {
            Some(Reg::Ok) => Err("le registre ok est en lecture seule".into()),
            Some(r) => Ok(r),
            None => Err(format!("« {} » n'est pas un registre (a, b, c ou d)", args[i])),
        }
    };
    let val = |i: usize| parse_val(args[i]);
    let label = |i: usize| -> Result<u16, String> {
        let key = args[i].to_ascii_lowercase();
        labels
            .iter()
            .find(|(n, _, _)| *n == key)
            .map(|(_, at, _)| *at)
            .ok_or_else(|| format!("étiquette inconnue « {} »", args[i]))
    };
    type Res = Result<Instr, String>;
    let arith = |op: Arith| -> Res { Ok(Instr::Arith(op, reg(0)?, val(1)?)) };
    let jump2 = |c: Cond| -> Res { Ok(Instr::Jump(c, val(0)?, val(1)?, label(2)?)) };
    let sense = |s: Sensor| -> Res { Ok(Instr::Sense(s, reg(0)?, val(1)?)) };
    let act = |a: Action| -> Res { Ok(Instr::Act(a, val(0)?)) };
    match mn.name {
        "SET" => arith(Arith::Set),
        "ADD" => arith(Arith::Add),
        "SUB" => arith(Arith::Sub),
        "MUL" => arith(Arith::Mul),
        "DIV" => arith(Arith::Div),
        "MOD" => arith(Arith::Mod),
        "RAND" => arith(Arith::Rand),
        "INC" => Ok(Instr::Inc(reg(0)?)),
        "DEC" => Ok(Instr::Dec(reg(0)?)),
        "JMP" => Ok(Instr::Jump(Cond::Always, Val::K(0), Val::K(0), label(0)?)),
        "JZ" => Ok(Instr::Jump(Cond::Z, val(0)?, Val::K(0), label(1)?)),
        "JNZ" => Ok(Instr::Jump(Cond::Nz, val(0)?, Val::K(0), label(1)?)),
        "JEQ" => jump2(Cond::Eq),
        "JNE" => jump2(Cond::Ne),
        "JLT" => jump2(Cond::Lt),
        "JGT" => jump2(Cond::Gt),
        "JLE" => jump2(Cond::Le),
        "JGE" => jump2(Cond::Ge),
        "CALL" => Ok(Instr::Call(label(0)?)),
        "RET" => Ok(Instr::Ret),
        "SCAN" => sense(Sensor::Scan),
        "DIST" => sense(Sensor::Dist),
        "DX" => sense(Sensor::Dx),
        "DY" => sense(Sensor::Dy),
        "COUNT" => sense(Sensor::Count),
        "LOOK" => sense(Sensor::Look),
        "CARGO" => Ok(Instr::Sense(Sensor::Cargo, reg(0)?, Val::K(0))),
        "HP" => Ok(Instr::Sense(Sensor::Hp, reg(0)?, Val::K(0))),
        "POSX" => Ok(Instr::Sense(Sensor::PosX, reg(0)?, Val::K(0))),
        "POSY" => Ok(Instr::Sense(Sensor::PosY, reg(0)?, Val::K(0))),
        "ANGLE" => Ok(Instr::Angle(reg(0)?, val(1)?, val(2)?)),
        "PEEK" => Ok(Instr::Peek(reg(0)?, val(1)?)),
        "POKE" => Ok(Instr::Poke(val(0)?, val(1)?)),
        "MOVE" => act(Action::Move),
        "MINE" => Ok(Instr::Act(Action::Mine, Val::K(0))),
        "DROP" => Ok(Instr::Act(Action::Drop, Val::K(0))),
        "ATTACK" => Ok(Instr::Act(Action::Attack, Val::K(0))),
        "WAIT" => Ok(Instr::Act(Action::Wait, Val::K(0))),
        other => Err(format!("instruction non gérée {other}")),
    }
}

// ---------------------------------------------------------------------------
// Interpreter
// ---------------------------------------------------------------------------

/// What the VM can ask the world while it thinks.
pub trait Senses {
    fn sense(&self, sensor: Sensor, arg: i32) -> i32;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vm {
    pub pc: u16,
    pub regs: [i32; 4],
    pub ok: i32,
    pub stack: [u16; STACK_DEPTH],
    pub sp: u8,
    pub rng: u64,
    /// Instruction that ended the last tick (for the debugger).
    pub last_pc: u16,
    /// Instructions executed during the last tick.
    pub last_ops: u8,
    pub stalled: bool,
}

impl Vm {
    pub fn new(seed: u64) -> Vm {
        Vm {
            pc: 0,
            regs: [0; 4],
            ok: 1,
            stack: [0; STACK_DEPTH],
            sp: 0,
            rng: seed,
            last_pc: 0,
            last_ops: 0,
            stalled: false,
        }
    }

    /// Restart from the top (used when a dronoid is reprogrammed).
    pub fn reset(&mut self) {
        let rng = self.rng;
        *self = Vm::new(rng);
    }

    pub fn reg(&self, r: Reg) -> i32 {
        match r {
            Reg::A => self.regs[0],
            Reg::B => self.regs[1],
            Reg::C => self.regs[2],
            Reg::D => self.regs[3],
            Reg::Ok => self.ok,
        }
    }

    fn set(&mut self, r: Reg, v: i32) {
        match r {
            Reg::A => self.regs[0] = v,
            Reg::B => self.regs[1] = v,
            Reg::C => self.regs[2] = v,
            Reg::D => self.regs[3] = v,
            Reg::Ok => {}
        }
    }

    pub fn get(&self, v: Val) -> i32 {
        match v {
            Val::R(r) => self.reg(r),
            Val::K(k) => k,
        }
    }

    /// Run until an action. `None` means the dronoid ran out of thinking
    /// budget this tick (it simply waits, and resumes where it was).
    pub fn run(
        &mut self,
        prog: &Program,
        senses: &dyn Senses,
        mem: &mut [i32; HIVE_MEM],
    ) -> Option<(Action, i32)> {
        let len = prog.code.len();
        self.stalled = false;
        if len == 0 {
            self.last_ops = 0;
            return Some((Action::Wait, 0));
        }
        for ops in 1..=MAX_OPS_PER_TICK {
            if self.pc as usize >= len {
                self.pc = 0; // falling off the end loops back to the top
            }
            let here = self.pc;
            let ins = prog.code[here as usize];
            self.pc += 1;
            match ins {
                Instr::Arith(op, r, v) => {
                    let x = self.reg(r);
                    let y = self.get(v);
                    let out = match op {
                        Arith::Set => y,
                        Arith::Add => x.wrapping_add(y),
                        Arith::Sub => x.wrapping_sub(y),
                        Arith::Mul => x.wrapping_mul(y),
                        Arith::Div => {
                            if y == 0 {
                                0
                            } else {
                                x.wrapping_div(y)
                            }
                        }
                        Arith::Mod => {
                            if y == 0 {
                                0
                            } else {
                                x.wrapping_rem_euclid(y)
                            }
                        }
                        Arith::Rand => {
                            if y <= 0 {
                                0
                            } else {
                                (splitmix(&mut self.rng) % y as u64) as i32
                            }
                        }
                    };
                    self.set(r, out);
                }
                Instr::Inc(r) => self.set(r, self.reg(r).wrapping_add(1)),
                Instr::Dec(r) => self.set(r, self.reg(r).wrapping_sub(1)),
                Instr::Jump(c, a, b, to) => {
                    if c.eval(self.get(a), self.get(b)) {
                        self.pc = to;
                    }
                }
                Instr::Call(to) => {
                    if self.sp as usize == STACK_DEPTH {
                        // Overflow: forget the oldest return address.
                        self.stack.copy_within(1.., 0);
                        self.sp -= 1;
                    }
                    self.stack[self.sp as usize] = self.pc;
                    self.sp += 1;
                    self.pc = to;
                }
                Instr::Ret => {
                    if self.sp > 0 {
                        self.sp -= 1;
                        self.pc = self.stack[self.sp as usize];
                    } else {
                        self.pc = 0;
                    }
                }
                Instr::Sense(s, r, v) => {
                    let out = senses.sense(s, self.get(v));
                    self.set(r, out);
                }
                Instr::Angle(r, dx, dy) => {
                    let out = geo::compass(self.get(dx), self.get(dy));
                    self.set(r, out);
                }
                Instr::Peek(r, i) => {
                    let i = self.get(i);
                    let out = if (0..HIVE_MEM as i32).contains(&i) { mem[i as usize] } else { 0 };
                    self.set(r, out);
                }
                Instr::Poke(i, v) => {
                    let i = self.get(i);
                    if (0..HIVE_MEM as i32).contains(&i) {
                        mem[i as usize] = self.get(v);
                    }
                }
                Instr::Act(a, v) => {
                    self.last_pc = here;
                    self.last_ops = ops as u8;
                    return Some((a, self.get(v)));
                }
            }
        }
        self.last_pc = self.pc;
        self.last_ops = MAX_OPS_PER_TICK as u8;
        self.stalled = true;
        None
    }
}

// ---------------------------------------------------------------------------
// Lexing helpers for the editor's syntax highlighting
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokKind {
    Op(Cat),
    Reg,
    Const,
    Number,
    Label,
    LabelDef,
    Comment,
    Punct,
    Unknown,
}

/// Split a source line into (byte_start, byte_end, kind) spans.
pub fn highlight(line: &str) -> Vec<(usize, usize, TokKind)> {
    let mut out = vec![];
    let code_end = strip_comment(line).len();
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut seen_op = false;
    while i < code_end {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == ',' || c == ':' {
            out.push((i, i + 1, TokKind::Punct));
            i += 1;
            continue;
        }
        let start = i;
        while i < code_end {
            let ch = line[i..].chars().next().unwrap();
            if ch.is_whitespace() || ch == ',' || ch == ':' {
                break;
            }
            i += ch.len_utf8();
        }
        let word = &line[start..i];
        let is_def = line[i..code_end].trim_start().starts_with(':') && !seen_op;
        let kind = if is_def {
            TokKind::LabelDef
        } else if !seen_op {
            seen_op = true;
            match mnemonic(word) {
                Some(m) => TokKind::Op(m.cat),
                None => TokKind::Unknown,
            }
        } else if parse_reg(word).is_some() {
            TokKind::Reg
        } else if parse_int(word).is_some() {
            TokKind::Number
        } else if constant(word).is_some() {
            TokKind::Const
        } else {
            TokKind::Label
        };
        out.push((start, i, kind));
    }
    if code_end < line.len() {
        out.push((code_end, line.len(), TokKind::Comment));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl Senses for Fake {
        fn sense(&self, s: Sensor, arg: i32) -> i32 {
            match s {
                Sensor::Scan => DIR_E,
                Sensor::Cargo => 3,
                _ => arg,
            }
        }
    }

    #[test]
    fn assembles_and_runs() {
        let src = "start: SET a, 2\n ADD a, 3 ; five\nloop:\n  SCAN b, ORE\n  MOVE b\n  JMP loop\n";
        let p = assemble("t", src).unwrap();
        assert_eq!(p.code.len(), 5);
        let mut vm = Vm::new(1);
        let mut mem = [0; HIVE_MEM];
        assert_eq!(vm.run(&p, &Fake, &mut mem), Some((Action::Move, DIR_E)));
        assert_eq!(vm.regs[0], 5);
        assert_eq!(p.line_of(vm.last_pc), Some(4));
        assert_eq!(vm.run(&p, &Fake, &mut mem), Some((Action::Move, DIR_E)));
    }

    #[test]
    fn reports_errors_with_lines() {
        let src = "SET 3, a\nFOO\nJMP nowhere\nx: x:\nSET ok, 1\nMOVE";
        let errs = assemble("t", src).unwrap_err();
        let lines: Vec<usize> = errs.iter().map(|e| e.line).collect();
        assert_eq!(lines, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn infinite_loop_stalls() {
        let p = assemble("t", "l: JMP l").unwrap();
        let mut vm = Vm::new(1);
        let mut mem = [0; HIVE_MEM];
        assert_eq!(vm.run(&p, &Fake, &mut mem), None);
        assert!(vm.stalled);
    }

    #[test]
    fn call_ret_and_memory() {
        let src = "CALL f\nPEEK a, 3\nWAIT\nf: POKE 3, 42\nRET";
        let p = assemble("t", src).unwrap();
        let mut vm = Vm::new(1);
        let mut mem = [0; HIVE_MEM];
        assert_eq!(vm.run(&p, &Fake, &mut mem), Some((Action::Wait, 0)));
        assert_eq!(vm.regs[0], 42);
        assert_eq!(mem[3], 42);
    }

    #[test]
    fn mod_is_euclidean_and_div_by_zero_is_safe() {
        let p = assemble("t", "SET a, -1\nMOD a, 4\nSET b, 7\nDIV b, 0\nWAIT").unwrap();
        let mut vm = Vm::new(1);
        let mut mem = [0; HIVE_MEM];
        vm.run(&p, &Fake, &mut mem);
        assert_eq!(vm.regs[0], 3);
        assert_eq!(vm.regs[1], 0);
    }

    #[test]
    fn angle_instruction() {
        let p = assemble("t", "ANGLE a, 5, 5
ANGLE b, -3, 0
ANGLE c, 0, 0
MINE").unwrap();
        let mut vm = Vm::new(1);
        let mut mem = [0; HIVE_MEM];
        assert_eq!(vm.run(&p, &Fake, &mut mem), Some((Action::Mine, 0)));
        assert_eq!(vm.regs[..3], [135, 270, NONE]);
    }

    #[test]
    fn highlight_spans() {
        let spans = highlight("loop: MOVE a, N ; go");
        let kinds: Vec<TokKind> = spans.iter().map(|s| s.2).collect();
        assert_eq!(
            kinds,
            vec![
                TokKind::LabelDef,
                TokKind::Punct,
                TokKind::Op(Cat::Action),
                TokKind::Reg,
                TokKind::Punct,
                TokKind::Const,
                TokKind::Comment
            ]
        );
    }
}
