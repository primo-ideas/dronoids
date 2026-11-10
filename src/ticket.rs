//! Tickets: how the world server knows that a player is signed in on the
//! site. The site's account service (`site/dronoids_site.py`) and the world
//! server share a secret; the service hands a signed-in player
//! `site.<expiry>.<hmac>` for their name, which the game sends as its token.
//! The server only checks the signature: it never talks to the service.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// What a ticket starts with (a guest's own token never does).
pub const PREFIX: &str = "site.";
/// Stored in place of a guest token for a name that belongs to an account:
/// too short to be a guest token, so no guest can ever present it.
pub const OWNED: &str = "site";

fn mac(secret: &[u8], name: &str, expires: u64) -> Hmac<Sha256> {
    let mut m = Hmac::<Sha256>::new_from_slice(secret).expect("any key length works");
    m.update(format!("dronoids-ticket\n{name}\n{expires}").as_bytes());
    m
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// The ticket for `name`, good until `expires` (seconds since 1970).
pub fn sign(secret: &[u8], name: &str, expires: u64) -> String {
    let hex: String = mac(secret, name, expires).finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
    format!("{PREFIX}{expires}.{hex}")
}

/// Is this a ticket signed with `secret` for `name`, not yet expired?
pub fn verify(secret: &[u8], name: &str, ticket: &str, now: u64) -> bool {
    let Some((expires, hex)) = ticket.strip_prefix(PREFIX).and_then(|t| t.split_once('.')) else { return false };
    let (Ok(expires), Some(sig)) = (expires.parse::<u64>(), unhex(hex)) else { return false };
    expires > now && mac(secret, name, expires).verify_slice(&sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same values are checked by the account service's tests
    /// (`site/test_site.py`): both sides must sign alike.
    const SECRET: &[u8] = b"secret-de-test";
    const TICKET: &str = "site.2000000000.899a2517086a306754d5318670bb258950402f869c7c99fd89737247d15ec10c";

    #[test]
    fn a_ticket_is_good_for_its_name_until_it_expires() {
        let t = sign(SECRET, "Primo", 2_000_000_000);
        assert!(verify(SECRET, "Primo", &t, 1_999_999_999));
        assert!(!verify(SECRET, "Primo", &t, 2_000_000_000), "expired");
        assert!(!verify(SECRET, "Autre", &t, 0), "another name");
        assert!(!verify(b"autre-secret", "Primo", &t, 0), "another secret");
        // Stretching the expiry breaks the signature.
        assert!(!verify(SECRET, "Primo", &t.replace("2000000000", "3000000000"), 0));
        for bad in ["", "site.", "site.x.y", "site.2000000000.zz", "0123456789abcdef", "site.2000000000."] {
            assert!(!verify(SECRET, "Primo", bad, 0), "{bad}");
        }
    }

    #[test]
    fn the_account_service_signs_alike() {
        assert_eq!(sign(SECRET, "Primo", 2_000_000_000), TICKET);
    }
}
