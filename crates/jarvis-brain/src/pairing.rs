//! Fenêtre d'appairage ouverte par `jarvis pair` : code à 6 chiffres, usage unique,
//! 2 minutes. 3 essais au plus par adresse IP (elle est ensuite refusée jusqu'à la fin
//! de la fenêtre), 20 en tout (la fenêtre se ferme). Une machine du réseau ne peut donc
//! pas, à elle seule, fermer la fenêtre avant l'oreille attendue. Horloge injectée
//! (`now` en argument).

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

/// Durée d'ouverture de la fenêtre.
pub const PAIRING_TTL: Duration = Duration::from_secs(120);
/// Essais de code au plus, toutes adresses confondues : la fenêtre se ferme ensuite.
pub const MAX_ATTEMPTS: u8 = 20;
/// Essais de code au plus depuis une même adresse IP.
pub const MAX_ATTEMPTS_PER_IP: u8 = 3;

/// Nombre de codes possibles (6 chiffres).
const CODE_SPACE: u32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairAttempt {
    /// Code juste : rendu une seule fois, la fenêtre se ferme.
    Accepted,
    /// `attempts_left` : essais qui restent à cette adresse (0 : elle est refusée
    /// jusqu'à la fin de la fenêtre, ou la fenêtre est fermée).
    WrongCode { attempts_left: u8 },
    /// Cette adresse a épuisé ses essais : refusée sans regarder le code. La fenêtre
    /// reste ouverte pour les autres.
    Blocked,
    /// Fenêtre expirée, déjà utilisée ou essais épuisés.
    Closed,
}

pub struct PairingWindow {
    code: String,
    expires_at: Instant,
    attempts_left: u8,
    /// Essais faux par adresse (20 adresses au plus : chacune a coûté un essai).
    failures: HashMap<IpAddr, u8>,
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
            failures: HashMap::new(),
            used: false,
        };
        (window, code)
    }

    /// Essai de `code` venu de l'adresse `from`.
    pub fn try_code(&mut self, code: &str, from: IpAddr, now: Instant) -> PairAttempt {
        if !self.is_open(now) {
            return PairAttempt::Closed;
        }
        let from = from.to_canonical();
        let failed = self.failures.get(&from).copied().unwrap_or(0);
        if failed >= MAX_ATTEMPTS_PER_IP {
            return PairAttempt::Blocked;
        }
        // Longueurs différentes : `ct_eq` rend faux.
        if bool::from(self.code.as_bytes().ct_eq(code.as_bytes())) {
            self.used = true;
            return PairAttempt::Accepted;
        }
        self.attempts_left -= 1;
        self.failures.insert(from, failed + 1);
        PairAttempt::WrongCode {
            attempts_left: (MAX_ATTEMPTS_PER_IP - failed - 1).min(self.attempts_left),
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

    const ICI: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 30));

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, n))
    }

    fn wrong(code: &str) -> &'static str {
        if code == "000000" { "000001" } else { "000000" }
    }

    #[test]
    fn code_a_six_chiffres_usage_unique() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
        assert!(window.is_open(now));
        assert_eq!(window.expires_at(), now + PAIRING_TTL);
        assert_eq!(window.try_code(&code, ICI, now), PairAttempt::Accepted);
        assert!(!window.is_open(now));
        assert_eq!(window.try_code(&code, ICI, now), PairAttempt::Closed);
    }

    #[test]
    fn fenetre_expiree_refuse() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        assert!(window.is_open(now + PAIRING_TTL - Duration::from_millis(1)));
        let later = now + PAIRING_TTL;
        assert!(!window.is_open(later));
        assert_eq!(window.try_code(&code, ICI, later), PairAttempt::Closed);
    }

    #[test]
    fn trois_essais_par_adresse_puis_refusee() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        for left in (0..MAX_ATTEMPTS_PER_IP).rev() {
            assert_eq!(
                window.try_code(wrong(&code), ICI, now),
                PairAttempt::WrongCode {
                    attempts_left: left
                }
            );
        }
        // Même le bon code est refusé depuis cette adresse…
        assert_eq!(window.try_code(&code, ICI, now), PairAttempt::Blocked);
        // … IPv4 dans IPv6 compris.
        let mapped: IpAddr = "::ffff:192.168.1.30".parse().unwrap();
        assert_eq!(window.try_code(&code, mapped, now), PairAttempt::Blocked);
        // La fenêtre reste ouverte : l'oreille attendue s'appaire.
        assert!(window.is_open(now));
        assert_eq!(window.try_code(&code, ip(1), now), PairAttempt::Accepted);
    }

    #[test]
    fn vingt_essais_en_tout_ferment_la_fenetre() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        // 6 adresses × 3 essais, puis 2 de plus : 20.
        let mut tries = 0;
        'outer: for n in 0..10 {
            for _ in 0..MAX_ATTEMPTS_PER_IP {
                let attempt = window.try_code(wrong(&code), ip(n), now);
                tries += 1;
                let PairAttempt::WrongCode { attempts_left } = attempt else {
                    panic!("code faux attendu, reçu {attempt:?}");
                };
                if tries == MAX_ATTEMPTS {
                    assert_eq!(attempts_left, 0);
                    break 'outer;
                }
            }
        }
        assert!(!window.is_open(now));
        // Même le bon code, d'une adresse neuve, est refusé désormais.
        assert_eq!(window.try_code(&code, ip(99), now), PairAttempt::Closed);
    }

    #[test]
    fn essais_restants_bornes_par_le_total() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        for n in 0..(MAX_ATTEMPTS - 1) {
            let _ = window.try_code(wrong(&code), ip(n), now);
        }
        // Il reste un seul essai en tout : c'est ce qui est annoncé.
        assert_eq!(
            window.try_code(wrong(&code), ICI, now),
            PairAttempt::WrongCode { attempts_left: 0 }
        );
        assert!(!window.is_open(now));
    }

    #[test]
    fn code_mal_forme_compte_comme_un_essai() {
        let now = Instant::now();
        let (mut window, code) = PairingWindow::open(now, PAIRING_TTL);
        assert_eq!(
            window.try_code(&format!("{code}0"), ICI, now),
            PairAttempt::WrongCode {
                attempts_left: MAX_ATTEMPTS_PER_IP - 1
            }
        );
        assert_eq!(
            window.try_code("", ICI, now),
            PairAttempt::WrongCode {
                attempts_left: MAX_ATTEMPTS_PER_IP - 2
            }
        );
        assert_eq!(window.try_code(&code, ICI, now), PairAttempt::Accepted);
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
