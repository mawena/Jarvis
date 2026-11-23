//! Cerveau d'appoint : pour chaque phrase du mode local, Claude ou l'IA locale.
//!
//! Réglage `[cerveau] routage` : `local` (toujours l'IA locale), `claude` (toujours
//! Claude) ou `auto` (défaut). En `auto`, la phrase reste en local dès qu'un doute existe :
//! texte vide, mot sensible (fichiers personnels, secrets, santé, argent, mots ajoutés),
//! voix non reconnue, ou phrase dite à un micro de la maison sans ordre explicite
//! (« demande à Claude… »). Sinon, Claude.
//!
//! Si Claude échoue (quota, réseau, délai, réponse illisible), la même phrase part à
//! l'IA locale, sans message d'erreur : l'utilisateur n'entend que la réponse. Après un
//! échec, Claude n'est plus essayé pendant une pause (plus longue si aucun compte n'a de
//! quota) pour ne pas payer le délai à chaque phrase.

use super::{LlmBackend, LlmError, Understanding};
use crate::config::{CerveauConfig, Routage};
use jarvis_skills::ToolSpec;
use jarvis_skills::text::fold as normalize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;
use tracing::{info, warn};

/// D'où vient la phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origine<'a> {
    /// Dite à l'oreille `oreille` ; `reconnue` : voix vérifiée par l'empreinte.
    Voix { oreille: &'a str, reconnue: bool },
    /// Tapée (`jarvis say`, CLI).
    Tape,
}

/// Pourquoi une phrase reste en local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raison {
    /// `routage = "local"`.
    Force,
    /// Mot sensible trouvé : nom de la catégorie (« mots_sensibles » pour la liste libre).
    Sensible(String),
    /// Micro de la maison, sans ordre explicite.
    MicroMaison,
    /// Voix non reconnue : dans le doute.
    VoixInconnue,
    /// Phrase vide : dans le doute.
    Vide,
    /// Claude en pause après un échec récent.
    ClaudeEnPause,
}

/// Cerveau choisi pour une phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cible {
    Claude,
    Local(Raison),
}

/// Un mot ou une suite de mots normalisés ; `prefixe` : le dernier mot finissait par `*`.
#[derive(Debug, Clone)]
struct Motif {
    mots: Vec<String>,
    prefixe: bool,
}

impl Motif {
    fn new(texte: &str) -> Option<Self> {
        let t = texte.trim();
        let prefixe = t.ends_with('*');
        let mots: Vec<String> = normalize(t.trim_end_matches('*'))
            .split(' ')
            .filter(|m| !m.is_empty())
            .map(String::from)
            .collect();
        (!mots.is_empty()).then_some(Self { mots, prefixe })
    }

    /// Le motif apparaît-il, mot pour mot, dans `texte` (déjà découpé) ?
    fn dans(&self, texte: &[&str]) -> bool {
        let n = self.mots.len();
        if texte.len() < n {
            return false;
        }
        texte.windows(n).any(|w| {
            w.iter().zip(&self.mots).enumerate().all(|(i, (t, m))| {
                if self.prefixe && i == n - 1 {
                    t.starts_with(m.as_str())
                } else {
                    t == m
                }
            })
        })
    }
}

/// Règle de sensibilité et routage, sans état : testable seule.
#[derive(Debug, Clone)]
pub struct Regle {
    routage: Routage,
    micros_maison: bool,
    ordres: Vec<Motif>,
    oreilles_hors_maison: Vec<String>,
    /// (catégorie, motifs)
    sensibles: Vec<(String, Vec<Motif>)>,
}

impl Regle {
    pub fn new(cfg: &CerveauConfig) -> Self {
        let motifs = |l: &[String]| l.iter().filter_map(|m| Motif::new(m)).collect::<Vec<_>>();
        let mut sensibles: Vec<(String, Vec<Motif>)> = cfg
            .categories
            .0
            .iter()
            .map(|(nom, mots)| (nom.clone(), motifs(mots)))
            .collect();
        sensibles.push(("mots_sensibles".into(), motifs(&cfg.mots_sensibles)));
        sensibles.retain(|(_, m)| !m.is_empty());
        Self {
            routage: cfg.routage,
            micros_maison: cfg.micros_maison,
            ordres: motifs(&cfg.ordres_explicites),
            oreilles_hors_maison: cfg.oreilles_hors_maison.clone(),
            sensibles,
        }
    }

    pub fn routage(&self) -> Routage {
        self.routage
    }

    /// Catégorie sensible touchée par `texte`, s'il y en a une.
    pub fn sensible(&self, texte: &str) -> Option<&str> {
        let norm = normalize(texte);
        let mots: Vec<&str> = norm.split(' ').filter(|m| !m.is_empty()).collect();
        self.sensibles
            .iter()
            .find(|(_, motifs)| motifs.iter().any(|m| m.dans(&mots)))
            .map(|(nom, _)| nom.as_str())
    }

    /// La phrase demande-t-elle Claude exprès ?
    pub fn ordre_explicite(&self, texte: &str) -> bool {
        let norm = normalize(texte);
        let mots: Vec<&str> = norm.split(' ').filter(|m| !m.is_empty()).collect();
        self.ordres.iter().any(|m| m.dans(&mots))
    }

    /// Cerveau choisi pour `texte`, sans tenir compte d'une pause de Claude.
    pub fn choisir(&self, texte: &str, origine: Origine<'_>) -> Cible {
        match self.routage {
            Routage::Local => return Cible::Local(Raison::Force),
            Routage::Claude => return Cible::Claude,
            Routage::Auto => {}
        }
        if normalize(texte).is_empty() {
            return Cible::Local(Raison::Vide);
        }
        if let Some(cat) = self.sensible(texte) {
            return Cible::Local(Raison::Sensible(cat.to_string()));
        }
        if let Origine::Voix { oreille, reconnue } = origine {
            if !reconnue {
                return Cible::Local(Raison::VoixInconnue);
            }
            let maison = !self.oreilles_hors_maison.iter().any(|o| o == oreille);
            if self.micros_maison && maison && !self.ordre_explicite(texte) {
                return Cible::Local(Raison::MicroMaison);
            }
        }
        Cible::Claude
    }
}

/// Routeur avec repli : la règle, Claude, et la pause après un échec.
pub struct Appoint {
    regle: Regle,
    claude: Arc<dyn LlmBackend>,
    pause_jusqua: Mutex<Option<Instant>>,
    pause_quota: Duration,
    pause_echec: Duration,
}

impl Appoint {
    pub fn new(cfg: &CerveauConfig, claude: Arc<dyn LlmBackend>) -> Self {
        Self {
            regle: Regle::new(cfg),
            claude,
            pause_jusqua: Mutex::new(None),
            pause_quota: Duration::from_secs(cfg.claude.pause_sans_quota_min * 60),
            pause_echec: Duration::from_secs(cfg.claude.pause_apres_echec_s),
        }
    }

    pub fn regle(&self) -> &Regle {
        &self.regle
    }

    fn en_pause(&self) -> bool {
        let mut p = self.pause_jusqua.lock().unwrap();
        match *p {
            Some(fin) if Instant::now() < fin => true,
            Some(_) => {
                *p = None;
                false
            }
            None => false,
        }
    }

    fn mettre_en_pause(&self, duree: Duration) {
        if !duree.is_zero() {
            *self.pause_jusqua.lock().unwrap() = Some(Instant::now() + duree);
        }
    }

    /// Cerveau choisi, pause de Claude comprise.
    pub fn choisir(&self, texte: &str, origine: Origine<'_>) -> Cible {
        match self.regle.choisir(texte, origine) {
            Cible::Claude if self.en_pause() => Cible::Local(Raison::ClaudeEnPause),
            c => c,
        }
    }

    /// Compréhension par le cerveau choisi ; si Claude échoue, par `local`. Rend aussi
    /// le cerveau qui a finalement répondu.
    pub async fn comprendre(
        &self,
        texte: &str,
        origine: Origine<'_>,
        tools: &[ToolSpec],
        local: &dyn LlmBackend,
    ) -> (Result<Understanding, LlmError>, Cible) {
        let cible = self.choisir(texte, origine);
        if let Cible::Local(raison) = &cible {
            info!(?raison, "cerveau : IA locale");
            return (local.understand(texte, tools).await, cible);
        }
        info!("cerveau : Claude d'appoint");
        match self.claude.understand(texte, tools).await {
            Ok(u) => (Ok(u), Cible::Claude),
            Err(e) => {
                let pause = match e {
                    LlmError::Quota(_) => self.pause_quota,
                    _ => self.pause_echec,
                };
                warn!(erreur = %e, pause_s = pause.as_secs(), "Claude indisponible : repli sur l'IA locale");
                self.mettre_en_pause(pause);
                (
                    local.understand(texte, tools).await,
                    Cible::Local(Raison::ClaudeEnPause),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeLlm;

    /// Les tests de la règle partent du mode `auto` (le défaut est `local`).
    fn auto() -> CerveauConfig {
        CerveauConfig { routage: Routage::Auto, ..CerveauConfig::default() }
    }

    fn regle(f: impl FnOnce(&mut CerveauConfig)) -> Regle {
        let mut cfg = auto();
        f(&mut cfg);
        Regle::new(&cfg)
    }

    const TAPE: Origine<'static> = Origine::Tape;

    fn voix(oreille: &str) -> Origine<'_> {
        Origine::Voix {
            oreille,
            reconnue: true,
        }
    }

    fn sensible(cat: &str) -> Cible {
        Cible::Local(Raison::Sensible(cat.into()))
    }

    #[test]
    fn phrases_sensibles_restent_locales() {
        let r = regle(|_| {});
        for (p, cat) in [
            ("ouvre le fichier rapport.pdf", "fichiers_personnels"),
            ("lis mes mails", "fichiers_personnels"),
            ("quel est mon mot de passe wifi", "secrets"),
            ("Où est ma clé SSH ?", "secrets"),
            ("j'ai mal à la tête, que prendre ?", "sante"),
            ("quels médicaments pour la grippe", "sante"),
            ("combien j'ai sur mon compte en banque", "argent"),
            ("rappelle-moi de payer le loyer", "argent"),
            ("Combien coûte 30 euros en FCFA ?", "argent"),
        ] {
            assert_eq!(r.choisir(p, TAPE), sensible(cat), "{p}");
        }
    }

    #[test]
    fn phrases_anodines_vont_a_claude() {
        let r = regle(|_| {});
        for p in [
            "quelle est la capitale du Pérou",
            "raconte-moi une blague",
            "explique la photosynthèse",
            "parle-moi de l'Europe",
            "écris un poème sur la mer",
        ] {
            assert_eq!(r.choisir(p, TAPE), Cible::Claude, "{p}");
        }
    }

    #[test]
    fn mot_entier_seulement() {
        let r = regle(|_| {});
        // « pin » ne touche pas « sapin », « api » pas « rapide », « euro » pas « Europe ».
        assert_eq!(r.choisir("décore le sapin", TAPE), Cible::Claude);
        assert_eq!(r.choisir("une recette rapide", TAPE), Cible::Claude);
        assert_eq!(r.choisir("histoire de l'Europe", TAPE), Cible::Claude);
    }

    #[test]
    fn micro_maison_local_sauf_ordre_explicite() {
        let r = regle(|_| {});
        assert_eq!(
            r.choisir("quelle est la capitale du Pérou", voix("salon")),
            Cible::Local(Raison::MicroMaison)
        );
        assert_eq!(
            r.choisir("demande à Claude la capitale du Pérou", voix("salon")),
            Cible::Claude
        );
        // L'ordre explicite ne lève jamais la sensibilité.
        assert_eq!(
            r.choisir("demande à Claude mon mot de passe", voix("salon")),
            sensible("secrets")
        );
    }

    #[test]
    fn oreille_hors_maison_et_voix_inconnue() {
        let r = regle(|c| c.oreilles_hors_maison = vec!["pixel-ccs".into()]);
        assert_eq!(
            r.choisir("raconte une blague", voix("pixel-ccs")),
            Cible::Claude
        );
        assert_eq!(
            r.choisir(
                "demande à Claude une blague",
                Origine::Voix {
                    oreille: "pixel-ccs",
                    reconnue: false
                }
            ),
            Cible::Local(Raison::VoixInconnue)
        );
        let r = regle(|c| c.micros_maison = false);
        assert_eq!(
            r.choisir("raconte une blague", voix("salon")),
            Cible::Claude
        );
    }

    #[test]
    fn forcages_et_doute() {
        let r = regle(|c| c.routage = Routage::Local);
        assert_eq!(
            r.choisir("raconte une blague", TAPE),
            Cible::Local(Raison::Force)
        );
        let r = regle(|c| c.routage = Routage::Claude);
        assert_eq!(r.choisir("mon mot de passe", voix("salon")), Cible::Claude);
        let r = regle(|_| {});
        assert_eq!(r.choisir("  ", TAPE), Cible::Local(Raison::Vide));
    }

    #[test]
    fn categories_et_mots_configurables() {
        let r = regle(|c| {
            c.categories.0.insert("argent".into(), Vec::new());
            c.categories
                .0
                .insert("famille".into(), vec!["ma sœur".into(), "cousin*".into()]);
            c.mots_sensibles = vec!["Projet Zèbre".into()];
        });
        assert_eq!(r.choisir("parle-moi d'argent", TAPE), Cible::Claude);
        assert_eq!(r.choisir("appelle ma sœur", TAPE), sensible("famille"));
        assert_eq!(r.choisir("mes cousines", TAPE), sensible("famille"));
        assert_eq!(
            r.choisir("où en est le projet zebre", TAPE),
            sensible("mots_sensibles")
        );
    }

    fn reponse(s: &str) -> FakeLlm {
        FakeLlm::new(Ok(Understanding::Answer(s.into())))
    }

    #[tokio::test]
    async fn claude_repond() {
        let claude = Arc::new(reponse("de Claude"));
        let local = reponse("du local");
        let a = Appoint::new(&auto(), claude.clone());
        let (r, c) = a.comprendre("raconte une blague", TAPE, &[], &local).await;
        assert_eq!(r.unwrap(), Understanding::Answer("de Claude".into()));
        assert_eq!(c, Cible::Claude);
        assert_eq!((claude.calls(), local.calls()), (1, 0));
    }

    #[tokio::test]
    async fn sensible_ne_va_jamais_a_claude() {
        let claude = Arc::new(reponse("de Claude"));
        let local = reponse("du local");
        let a = Appoint::new(&auto(), claude.clone());
        let (r, _) = a.comprendre("mon mot de passe", TAPE, &[], &local).await;
        assert_eq!(r.unwrap(), Understanding::Answer("du local".into()));
        assert_eq!((claude.calls(), local.calls()), (0, 1));
    }

    #[tokio::test(start_paused = true)]
    async fn repli_sans_erreur_puis_pause_quota() {
        let claude = Arc::new(FakeLlm::new(Err(LlmError::Quota("épuisé".into()))));
        let local = reponse("du local");
        let a = Appoint::new(&auto(), claude.clone());
        let (r, c) = a.comprendre("raconte une blague", TAPE, &[], &local).await;
        // L'utilisateur reçoit la réponse locale, pas l'erreur.
        assert_eq!(r.unwrap(), Understanding::Answer("du local".into()));
        assert_eq!(c, Cible::Local(Raison::ClaudeEnPause));
        // Pendant la pause (10 min), Claude n'est plus appelé.
        tokio::time::advance(Duration::from_secs(9 * 60)).await;
        let _ = a.comprendre("raconte une blague", TAPE, &[], &local).await;
        assert_eq!((claude.calls(), local.calls()), (1, 2));
        // Après la pause, il est réessayé.
        tokio::time::advance(Duration::from_secs(2 * 60)).await;
        let _ = a.comprendre("raconte une blague", TAPE, &[], &local).await;
        assert_eq!((claude.calls(), local.calls()), (2, 3));
    }

    #[tokio::test(start_paused = true)]
    async fn echec_reseau_pause_courte() {
        let claude = Arc::new(FakeLlm::new(Err(LlmError::Unreachable("délai".into()))));
        let local = reponse("du local");
        let a = Appoint::new(&auto(), claude.clone());
        let (r, _) = a.comprendre("raconte une blague", TAPE, &[], &local).await;
        assert!(r.is_ok());
        tokio::time::advance(Duration::from_secs(61)).await;
        let _ = a.comprendre("raconte une blague", TAPE, &[], &local).await;
        assert_eq!((claude.calls(), local.calls()), (2, 2));
    }

    #[tokio::test]
    async fn erreur_locale_remonte_telle_quelle() {
        let claude = Arc::new(FakeLlm::new(Err(LlmError::Unreachable("x".into()))));
        let local = FakeLlm::new(Err(LlmError::Unreachable("LM Studio".into())));
        let a = Appoint::new(&auto(), claude);
        let (r, c) = a.comprendre("raconte une blague", TAPE, &[], &local).await;
        assert!(matches!(r, Err(LlmError::Unreachable(_))));
        assert!(matches!(c, Cible::Local(_)));
    }
}
