//! Fenêtre d'appairage ouverte par `jarvis pair` : code à 6 chiffres, usage unique,
//! 5 essais au plus, 2 minutes. Horloge injectée (`now` en argument).

use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

/// Durée d'ouverture de la fenêtre.
pub const PAIRING_TTL: Duration = Duration::from_secs(120);
/// Essais de code au plus.
pub const MAX_ATTEMPTS: u8 = 5;

/// Nombre de codes possibles (6 chiffres).
const CODE_SPACE: u32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairAttempt {
    /// Code juste : rendu une seule fois, la fenêtre se ferme.
    Accepted,
    WrongCode {
        attempts_left: u8,
    },
    /// Fenêtre expirée, déjà utilisée ou essais épuisés.
    Closed,
}

pub struct PairingWindow {
    code: String,
    expires_at: Instant,
    attempts_left: u8,
    used: bool,
}

impl std::fmt::Debug for PairingWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Le code n'apparaît jamais dans les journaux.
        f.debug_struct("PairingWindow")
            .field("expires_at", &self.expires_at)
            .field("attempts_left", &self.attempts_left)
            .field("used", &self.used)
            .finish_non_exhaustive()
    }
}

impl PairingWindow {
    /// Ouvre une fenêtre ; rend aussi le code (tirage sans biais).
    pub fn open(now: Instant, ttl: Duration) -> (Self, String) {
        let code = format!("{:06}", random_code());
        let window = PairingWindow {
            code: code.clone(),
            expires_at: now + ttl,
            attempts_left: MAX_ATTEMPTS,
            used: false,
        };
        (window, code)
    }

    pub fn try_code(&mut self, code: &str, now: Instant) -> PairAttempt {
        if !self.is_open(now) {
            return PairAttempt::Closed;
        }
        // Longueurs différentes : `ct_eq` rend faux.
        if bool::from(self.code.as_bytes().ct_eq(code.as_bytes())) {
            self.used = true;
            return PairAttempt::Accepted;
        }
        self.attempts_left -= 1;
        PairAttempt::WrongCode {
            attempts_left: self.attempts_left,
        }
    }

    pub fn is_open(&self, now: Instant) -> bool {
        !self.used && self.attempts_left > 0 && now < self.expires_at
    }

    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }
}

/// Tirage uniforme dans `0..CODE_SPACE` (la queue est rejetée : pas de biais du modulo).
fn random_code() -> u32 {
    let zone = u32::MAX - u32::MAX % CODE_SPACE;
    loop {
        let v = getrandom::u32().expect("source d'aléa du système indisponible");
        if v < zone {
            return v % CODE_SPACE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_a_six_chiffres_usage_unique() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
        assert!(window.is_open(now));
        assert_eq!(window.expires_at(), now + PAIRING_TTL);
        assert_eq!(window.try_code(&code, now), PairAttempt::Accepted);
        assert!(!window.is_open(now));
        assert_eq!(window.try_code(&code, now), PairAttempt::Closed);
    }

    #[test]
    fn fenetre_expiree_refuse() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        assert!(window.is_open(now + PAIRING_TTL - Duration::from_millis(1)));
        let later = now + PAIRING_TTL;
        assert!(!window.is_open(later));
        assert_eq!(window.try_code(&code, later), PairAttempt::Closed);
    }

    #[test]
    fn trop_d_essais_ferme_la_fenetre() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        let wrong = if code == "000000" { "000001" } else { "000000" };
        for left in (0..MAX_ATTEMPTS).rev() {
            assert_eq!(
                window.try_code(wrong, now),
                PairAttempt::WrongCode {
                    attempts_left: left
                }
            );
        }
        assert!(!window.is_open(now));
        // Même le bon code est refusé désormais.
        assert_eq!(window.try_code(&code, now), PairAttempt::Closed);
    }

    #[test]
    fn code_mal_forme_compte_comme_un_essai() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        assert_eq!(
            window.try_code(&format!("{code}0"), now),
            PairAttempt::WrongCode {
                attempts_left: MAX_ATTEMPTS - 1
            }
        );
        assert_eq!(
            window.try_code("", now),
            PairAttempt::WrongCode {
                attempts_left: MAX_ATTEMPTS - 2
            }
        );
        assert_eq!(window.try_code(&code, now), PairAttempt::Accepted);
    }

    #[test]
    fn codes_tires_varies() {
        let now = Instant::now();
        let codes: std::collections::HashSet<String> = (0..20)
            .map(|_| PairingWindow::open(now, PAIRING_TTL).1)
            .collect();
        assert!(codes.len() > 10);
    }
}
