//! Built-in DASM programs. They are intentionally naive: the fun of the game
//! is to write better ones.

pub struct Builtin {
    pub name: &'static str,
    pub summary: &'static str,
    pub source: &'static str,
}

pub const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "mineur",
        summary: "Va au cristal le plus proche, remplit sa soute, rentre.",
        source: r#"; MINEUR -- le programme de départ.
; Va au cristal le plus proche, le creuse jusqu'à
; remplir sa soute, puis rentre décharger à l'usine.
; Défaut : fonce tout droit et se cogne aux astéroïdes.

debut:
    CARGO a
    JGE a, FULL, rentrer
    DIST a, ORE          ; distance au cristal le plus proche
    JEQ a, NONE, errer   ; rien en vue ?
    JLE a, 1, creuser    ; à portée : on creuse
    SCAN b, ORE          ; b = angle vers le cristal
    MOVE b
    JZ ok, esquive       ; bloqué
    JMP debut

creuser:
    MINE
    JMP debut

rentrer:
    DIST a, BASE
    JLE a, 1, vider
    SCAN b, BASE
    MOVE b
    JZ ok, esquive
    JMP rentrer

vider:
    DROP
    JMP debut

esquive:                 ; un pas de côté au hasard
    RAND c, 180
    ADD b, c
    SUB b, 90
    MOVE b
    JMP debut

errer:
    RAND b, 360
    MOVE b
    MOVE b
    MOVE b
    JMP debut
"#,
    },
    Builtin {
        name: "eclaireur",
        summary: "Part en ligne droite, change de cap quand il bute.",
        source: r#"; ECLAIREUR -- part en ligne droite et change de cap
; quand il bute. Mine ce qu'il croise, rentre quand il
; est plein. Utile pour découvrir des gisements lointains
; et étendre la zone où l'on peut construire.

    RAND d, 360          ; d = cap
boucle:
    CARGO a
    JGE a, FULL, rentrer
    DIST a, ORE
    JEQ a, NONE, avancer
    JLE a, 1, creuser
avancer:
    MOVE d
    JNZ ok, boucle
    RAND d, 360          ; bloqué : nouveau cap
    JMP boucle

creuser:
    MINE
    JMP boucle

rentrer:
    DIST a, BASE
    JLE a, 1, vider
    SCAN b, BASE
    MOVE b
    JNZ ok, rentrer
    RAND c, 180
    ADD b, c
    SUB b, 90
    MOVE b
    JMP rentrer

vider:
    DROP
    RAND d, 360
    JMP boucle
"#,
    },
    Builtin {
        name: "chasseur",
        summary: "Traque les ennemis, sinon file vers la cible de l'usine.",
        source: r#"; CHASSEUR -- traque les dronoids ennemis à portée,
; sinon attaque l'usine ennemie la plus proche
; (repérée jusqu'à 40 unités), sinon file vers la
; cible de son usine : mémoire 0 = X, 1 = Y, 2 = 1
; si une cible est posée. Sinon, patrouille.

chasse:
    DIST a, ENEMY
    JEQ a, NONE, siege
    JLE a, 2, feu        ; à portée de tir
    SCAN b, ENEMY
    MOVE b
    JZ ok, esquive
    JMP chasse

siege:
    DIST a, EBASE
    JEQ a, NONE, cible
    JLE a, 2, feu
    SCAN b, EBASE
    MOVE b
    JZ ok, esquive
    JMP chasse

feu:
    ATTACK
    JMP chasse

cible:
    PEEK a, 2
    JZ a, patrouille     ; aucune cible posée
    POSX a
    PEEK b, 0
    SUB b, a             ; b = écart est-ouest
    POSY a
    PEEK c, 1
    SUB c, a             ; c = écart nord-sud
    ANGLE b, b, c
    JEQ b, NONE, patrouille
    MOVE b
    JZ ok, esquive
    JMP chasse

patrouille:
    JNZ d, cap           ; d = cap (0 tant qu'il n'est pas choisi)
    RAND d, 359
    INC d
cap:
    MOVE d
    JNZ ok, chasse
    RAND d, 359
    INC d
    JMP chasse

esquive:
    RAND c, 180
    ADD b, c
    SUB b, 90
    MOVE b
    JMP chasse
"#,
    },
    Builtin {
        name: "gardien",
        summary: "Reste près de la base et tire sur les intrus.",
        source: r#"; GARDIEN -- reste près de son usine et
; tire sur tout ennemi qui s'approche.

garde:
    DIST a, ENEMY
    JEQ a, NONE, retour
    JLE a, 2, feu
    JGT a, 8, retour     ; trop loin : on ne lâche pas la base
    SCAN b, ENEMY
    MOVE b
    JMP garde

feu:
    ATTACK
    JMP garde

retour:
    DIST a, BASE
    JLE a, 3, veille
    SCAN b, BASE
    MOVE b
    JNZ ok, garde
    RAND c, 180
    ADD b, c
    SUB b, 90
    MOVE b
    JMP garde

veille:
    WAIT
    JMP garde
"#,
    },
];

/// Template for a brand-new program.
pub const TEMPLATE: &str = r#"; Nouveau programme.
; Registres : a b c d (et ok, mis à 1/0 par la dernière action)
; Angles en degrés : N=0 E=90 S=180 W=270
; Choses : ORE ALLY ENEMY BASE EBASE WALL EMPTY
; F1 : aide complète

debut:
    SCAN a, ORE
    MOVE a
    JMP debut
"#;

pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

/// Copies of the built-in programs kept by players and saved sectors still
/// have this comment in its former wording.
pub fn reworded(source: &str) -> String {
    source.replace("traque les drones ennemis", "traque les dronoids ennemis")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::assemble;

    #[test]
    fn an_old_copy_of_a_builtin_becomes_the_builtin_again() {
        let hunter = builtin("chasseur").unwrap().source;
        let old = hunter.replace("dronoids", "drones");
        assert_ne!(old, hunter);
        assert_eq!(reworded(&old), hunter);
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
