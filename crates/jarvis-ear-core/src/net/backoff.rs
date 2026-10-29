//! Délai entre deux tentatives de connexion : 500 ms, doublé à chaque échec,
//! plafonné à 30 s (5 min après un refus du jeton), ±20 % d'aléa.

use std::time::Duration;

pub const MIN_DELAY: Duration = Duration::from_millis(500);
pub const MAX_DELAY: Duration = Duration::from_secs(30);
/// Plafond après `Rejected { UnknownEar | BadToken }`.
pub const REJECTED_MAX_DELAY: Duration = Duration::from_secs(300);

pub struct Backoff {
    min: Duration,
    max: Duration,
    factor: u32,
    /// Part d'aléa (0,2 : ±20 %).
    jitter: f64,
    current: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        Backoff {
            min: MIN_DELAY,
            max: MAX_DELAY,
            factor: 2,
            jitter: 0.2,
            current: MIN_DELAY,
        }
    }

    /// Délai avant la prochaine tentative.
    pub fn next_delay(&mut self) -> Duration {
        let base = self.current.min(self.max);
        self.current = base.saturating_mul(self.factor).min(self.max);
        // Aléa dans [−jitter, +jitter], sans dépasser le plafond.
        let unit = f64::from(getrandom::u32().unwrap_or(u32::MAX / 2)) / f64::from(u32::MAX);
        let scale = 1.0 + self.jitter * (2.0 * unit - 1.0);
        base.mul_f64(scale).min(self.max)
    }

    /// Connexion réussie : on repart du minimum, plafond d'origine.
    pub fn reset(&mut self) {
        self.current = self.min;
        self.max = MAX_DELAY;
    }

    /// Change le plafond (refus du jeton : [`REJECTED_MAX_DELAY`]).
    pub fn set_max(&mut self, max: Duration) {
        self.max = max;
        self.current = self.current.min(max);
    }

    /// Refus du jeton : plafond de 5 min, et pas moins de [`MAX_DELAY`] d'attente
    /// (inutile d'insister, il faut réappairer).
    pub fn rejected(&mut self) {
        self.set_max(REJECTED_MAX_DELAY);
        self.current = self.current.max(MAX_DELAY);
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within(d: Duration, base: Duration) -> bool {
        d >= base.mul_f64(0.8) && d <= base.mul_f64(1.2)
    }

    #[test]
    fn delai_croissant_plafonne() {
        let mut b = Backoff::new();
        let mut expected = MIN_DELAY;
        for _ in 0..12 {
            let d = b.next_delay();
            assert!(within(d, expected), "{d:?} vs {expected:?}");
            assert!(d <= MAX_DELAY);
            expected = (expected * 2).min(MAX_DELAY);
        }
        // Au plafond : entre 24 et 30 s.
        let d = b.next_delay();
        assert!(d >= MAX_DELAY.mul_f64(0.8) && d <= MAX_DELAY, "{d:?}");
        // L'aléa varie.
        let delays: Vec<_> = (0..20).map(|_| b.next_delay()).collect();
        assert!(delays.iter().any(|d| *d != delays[0]));
    }

    #[test]
    fn delai_remis_a_zero_apres_connexion() {
        let mut b = Backoff::new();
        for _ in 0..8 {
            b.next_delay();
        }
        assert!(b.next_delay() > Duration::from_secs(10));
        b.reset();
        assert!(within(b.next_delay(), MIN_DELAY));
        // Après un refus du jeton : au moins ~30 s, jusqu'à 5 min.
        b.rejected();
        let first = b.next_delay();
        assert!(within(first, MAX_DELAY), "{first:?}");
        for _ in 0..10 {
            b.next_delay();
        }
        let d = b.next_delay();
        assert!(d >= REJECTED_MAX_DELAY.mul_f64(0.8) && d <= REJECTED_MAX_DELAY);
        // Connexion réussie : plafond d'origine.
        b.reset();
        for _ in 0..20 {
            assert!(b.next_delay() <= MAX_DELAY);
        }
        b.set_max(Duration::from_secs(2));
        assert!(b.next_delay() <= Duration::from_secs(2));
    }
}
