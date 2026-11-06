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
        summary: "Va au minerai le plus proche, remplit sa soute, rentre.",
        source: r#"; MINEUR -- le programme de départ.
; Cherche le minerai le plus proche, le creuse jusqu'à
; remplir sa soute, puis rentre décharger à l'usine.
; Défaut : fonce tout droit et se cogne aux murs.

debut:
    CARGO a
    JGE a, FULL, rentrer
    SCAN b, ORE          ; b = direction du minerai
    JEQ b, NONE, errer   ; rien en vue ?
    LOOK c, b
    JEQ c, ORE, creuser
    MOVE b
    JZ ok, errer         ; bloqué : pas de côté au hasard
    JMP debut

creuser:
    MINE b
    JMP debut

rentrer:
    SCAN b, BASE
    JEQ b, NONE, errer
    LOOK c, b
    JEQ c, BASE, vider
    MOVE b
    JZ ok, errer
    JMP debut

vider:
    DROP b
    JMP debut

errer:
    RAND b, 4
    MOVE b
    JMP debut
"#,
    },
    Builtin {
        name: "eclaireur",
        summary: "File en ligne droite, change de cap quand il bute.",
        source: r#"; ECLAIREUR -- explore en ligne droite et tourne
; quand il bute. Mine ce qu'il croise et rapporte
; quand il est plein. Utile pour révéler des gisements
; et étendre la zone où l'on peut construire.

    RAND d, 4            ; d = cap
boucle:
    CARGO a
    JGE a, FULL, rentrer
    LOOK c, d
    JEQ c, ORE, creuser
    MOVE d
    JNZ ok, boucle
    RAND d, 4            ; bloqué : nouveau cap
    JMP boucle

creuser:
    MINE d
    JMP boucle

rentrer:
    SCAN b, BASE
    LOOK c, b
    JEQ c, BASE, vider
    MOVE b
    JNZ ok, rentrer
    RAND b, 4
    MOVE b
    JMP rentrer

vider:
    DROP b
    RAND d, 4
    JMP boucle
"#,
    },
    Builtin {
        name: "chasseur",
        summary: "Traque les drones ennemis puis assiège leurs usines.",
        source: r#"; CHASSEUR -- traque les drones ennemis à portée,
; sinon fonce sur l'usine ennemie la plus proche.

chasse:
    SCAN b, ENEMY
    JEQ b, NONE, siege
    LOOK c, b
    JEQ c, ENEMY, frappe
    MOVE b
    JZ ok, esquive
    JMP chasse

siege:
    SCAN b, EBASE
    JEQ b, NONE, patrouille
    LOOK c, b
    JEQ c, EBASE, frappe
    MOVE b
    JZ ok, esquive
    JMP chasse

frappe:
    ATTACK b
    JMP chasse

patrouille:
    RAND b, 4
    MOVE b
    JMP chasse

esquive:
    RAND b, 4
    MOVE b
    JMP chasse
"#,
    },
    Builtin {
        name: "gardien",
        summary: "Reste près de la base et attaque les intrus.",
        source: r#"; GARDIEN -- reste près de son usine et
; attaque tout ennemi qui s'approche.

garde:
    DIST a, ENEMY
    JEQ a, NONE, retour
    JGT a, 6, retour     ; trop loin, on ne lâche pas la base
    SCAN b, ENEMY
    LOOK c, b
    JEQ c, ENEMY, frappe
    MOVE b
    JMP garde

frappe:
    ATTACK b
    JMP garde

retour:
    DIST a, BASE
    JLE a, 2, veille
    SCAN b, BASE
    MOVE b
    JNZ ok, garde
    RAND b, 4
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
; Directions : N E S W   Choses : ORE ALLY ENEMY BASE EBASE WALL EMPTY
; F1 : aide complète

debut:
    SCAN a, ORE
    MOVE a
    JMP debut
"#;

pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::assemble;

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
