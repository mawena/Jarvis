//! Orchestrateur : raccourci du cerveau, puis raccourci d'une compétence, sinon IA
//! du mode courant, puis compétence.
//! Une action risquée attend d'abord un « oui », sur l'oreille qui a posé la question
//! (voir [`Brain::answer_follow_up`]).

use crate::fastpath::{BrainCommand, match_brain_command, match_confirmation, match_fast};
use crate::intent::Intent;
use crate::llm::{LlmBackend, LlmError, Understanding};
use crate::mode_store::ModeStore;
use crate::people::{Access, People, Speaker, Trust};
use crate::persona::Persona;
use crate::system_skill::SystemSkill;
use jarvis_protocol::{EarId, Mode};
use jarvis_skills::{Outcome, Reply, Risk, Skill, SkillError, ToolSpec};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Source de l'heure, remplaçable dans les tests.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Horloge réelle.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Réponse du cerveau. `awaits_reply` : une question est posée, l'oreille doit
/// écouter la réponse (oui / non) sans mot d'éveil. `fast_path` : la réponse vient
/// d'un raccourci (sans IA ni confirmation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub reply: Reply,
    pub awaits_reply: bool,
    pub fast_path: bool,
}

impl From<Reply> for Answer {
    fn from(reply: Reply) -> Self {
        Answer {
            reply,
            awaits_reply: false,
            fast_path: false,
        }
    }
}

/// Voix qui répond à une question (`Listen { follow_up: true }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyVoice {
    /// Voix de Charles vérifiée : phrase reconnue, « oui » ou « répète » court passé
    /// par la vérification stricte, ou aucune empreinte (installation neuve).
    Verified,
    /// Voix non vérifiée (phrase courte de session) : elle ne peut qu'annuler. Jamais
    /// elle n'exécute, ne repose la question, ne prolonge le délai ni ne redit rien.
    Unverified,
}

/// D'où vient la phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source<'a> {
    /// Voix, par l'oreille `ear`, de `who`. Voix « reconnue » : `who.trust ==
    /// Recognized` (empreinte, `Verdict::Match`). Seule une voix reconnue (ou le
    /// texte tapé) déclenche les raccourcis des compétences (l'agent de code).
    Voice { ear: &'a EarId, who: &'a Speaker },
    /// Texte tapé (`Say`, CLI) par `who` (le propriétaire) : la réponse n'est pas
    /// écoutée par l'oreille, donc jamais d'action risquée ni de question reposée.
    Typed { who: &'a Speaker },
}

impl Source<'_> {
    fn who(&self) -> &Speaker {
        match self {
            Source::Voice { who, .. } | Source::Typed { who } => who,
        }
    }

    /// Voix reconnue par l'empreinte, ou texte tapé.
    fn is_sure(&self) -> bool {
        match self {
            Source::Voice { who, .. } => who.trust == Trust::Recognized,
            Source::Typed { .. } => true,
        }
    }
}

/// Action risquée en attente d'un « oui ».
struct Pending {
    /// Oreille qui a posé la question : seule à pouvoir y répondre.
    ear: EarId,
    /// Personne qui a posé la question (pour le journal).
    person: String,
    intent: Intent,
    question: String,
    asked_at: Instant,
    /// Question déjà reposée (« répète » ou voix mal reconnue) : une seule fois par action.
    reasked: bool,
}

pub struct Brain {
    skills: Vec<Arc<dyn Skill>>,
    /// Compétence système : appelable par les raccourcis, jamais proposée à l'IA.
    system: Arc<SystemSkill>,
    local: Arc<dyn LlmBackend>,
    online: Arc<dyn LlmBackend>,
    modes: Arc<ModeStore>,
    skill_timeout: Duration,
    local_model: String,
    clock: Arc<dyn Clock>,
    /// Une seule action en attente à la fois, toutes oreilles confondues.
    pending: Mutex<Option<Pending>>,
    /// Dernière phrase dite (non vide), pour « répète ».
    last_speech: Mutex<Option<String>>,
    /// Ton des phrases fixes (nom d'adresse).
    persona: Persona,
    /// Annuaire des personnes et des rôles (phase 8).
    people: Arc<People>,
}

/// Résultat d'une intention.
enum Step {
    Answer(Answer),
    /// Cible introuvable : message prêt à être dit.
    NotFound(String),
}

impl Brain {
    pub fn new(
        skills: Vec<Arc<dyn Skill>>,
        local: Arc<dyn LlmBackend>,
        online: Arc<dyn LlmBackend>,
        modes: Arc<ModeStore>,
    ) -> Brain {
        Brain {
            skills,
            system: Arc::new(SystemSkill::new(modes.clone())),
            local,
            online,
            modes,
            skill_timeout: SKILL_TIMEOUT,
            local_model: String::new(),
            clock: Arc::new(SystemClock),
            pending: Mutex::new(None),
            last_speech: Mutex::new(None),
            persona: Persona::default(),
            people: Arc::new(People::admin_only("Charles")),
        }
    }

    /// Annuaire des personnes et des rôles (défaut : Charles `admin` seul).
    pub fn with_people(mut self, people: Arc<People>) -> Brain {
        self.people = people;
        self
    }

    pub fn people(&self) -> &Arc<People> {
        &self.people
    }

    /// Noms de tous les outils : compétences et système (validation des rôles).
    pub fn tool_names(&self) -> Vec<String> {
        let system: Arc<dyn Skill> = self.system.clone();
        self.skills
            .iter()
            .cloned()
            .chain(std::iter::once(system))
            .flat_map(|s| s.tools())
            .map(|t| t.name)
            .collect()
    }

    /// Ton de Jarvis (nom d'adresse des phrases fixes).
    pub fn with_persona(mut self, persona: Persona) -> Brain {
        self.persona = persona;
        self
    }

    pub fn persona(&self) -> &Persona {
        &self.persona
    }

    /// Remplace l'horloge (tests du délai de confirmation).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Brain {
        self.clock = clock;
        self
    }

    pub fn now(&self) -> Instant {
        self.clock.now()
    }

    /// Efface l'action en attente. Vrai s'il y en avait une.
    pub fn clear_pending(&self) -> bool {
        self.pending.lock().unwrap().take().is_some()
    }

    pub fn has_pending(&self) -> bool {
        self.pending.lock().unwrap().is_some()
    }

    /// Nom du modèle local, affiché par `Status`.
    pub fn with_local_model(mut self, model: impl Into<String>) -> Brain {
        self.local_model = model.into();
        self
    }

    pub fn local_model(&self) -> &str {
        &self.local_model
    }

    /// Change le délai maximal d'un appel de compétence (tests).
    pub fn with_skill_timeout(mut self, timeout: Duration) -> Brain {
        self.skill_timeout = timeout;
        self
    }

    /// Les outils proposés à l'IA : ceux que les compétences lui ouvrent
    /// (`llm_tools`), sans les outils système (`set_mode`, `status`), que les
    /// raccourcis couvrent.
    pub fn tools(&self) -> Vec<ToolSpec> {
        self.skills.iter().flat_map(|s| s.llm_tools()).collect()
    }

    pub fn mode(&self) -> Mode {
        self.modes.get()
    }

    pub fn set_mode(&self, mode: Mode) -> io::Result<()> {
        self.modes.set(mode)
    }

    /// Texte tapé (`Say`) : personne n'écoute la réponse, donc une action risquée n'est
    /// jamais mise en attente (elle se demande à voix haute) et « répète » ne repose pas
    /// la question en attente. Ne renvoie jamais d'erreur : tout échec devient une phrase.
    pub async fn handle_text(&self, text: &str) -> Reply {
        let who = self.people.typed();
        self.answer(text, Source::Typed { who: &who }).await.reply
    }

    /// Phrase complète d'une voix reconnue par son empreinte : commande du cerveau,
    /// raccourci, sinon IA. Toute nouvelle commande, de n'importe quelle oreille,
    /// efface l'action en attente (sauf « annule » et « répète »). `ear` : oreille qui
    /// a entendu la phrase.
    pub async fn answer_text(&self, ear: &EarId, text: &str) -> Answer {
        self.answer_voice(ear, text, true).await
    }

    /// Comme [`Self::answer_text`] ; `verified` faux (aucune empreinte enregistrée,
    /// vérification désactivée) : tout reste permis sauf les raccourcis des
    /// compétences, refusés d'une phrase sans repli sur l'IA.
    pub async fn answer_voice(&self, ear: &EarId, text: &str, verified: bool) -> Answer {
        let trust = if verified {
            Trust::Recognized
        } else {
            Trust::Accepted
        };
        let who = self.people.owner_speaker(trust);
        self.answer_as(ear, text, &who).await
    }

    /// Phrase complète de `who` (personne, rôle, confiance, droits), par l'oreille
    /// `ear`. Chaque outil est contrôlé contre ses droits avant d'être lancé ou
    /// demandé ; la liste d'outils proposée à l'IA ne change pas.
    pub async fn answer_as(&self, ear: &EarId, text: &str, who: &Speaker) -> Answer {
        self.answer(text, Source::Voice { ear, who }).await
    }

    async fn answer(&self, text: &str, source: Source<'_>) -> Answer {
        info!(phrase = %text, "phrase reçue");
        let answer = match match_brain_command(text) {
            Some(BrainCommand::Repeat) => {
                return match source {
                    Source::Voice { ear, .. } => self.repeat(ear),
                    Source::Typed { .. } => self.repeat_last(),
                };
            }
            Some(BrainCommand::Cancel) => self.cancel(),
            None => {
                self.clear_pending();
                self.understand(text, source).await
            }
        };
        self.remember(&answer);
        answer
    }

    async fn understand(&self, text: &str, source: Source<'_>) -> Answer {
        if let Some(intent) = match_fast(text) {
            debug!(route = "fast", tool = %intent.tool);
            match self.run_or_ask(intent, source).await {
                Step::Answer(a) => {
                    return Answer {
                        fast_path: true,
                        ..a
                    };
                }
                Step::NotFound(m) => {
                    debug!(message = %m, "raccourci sans résultat, repli sur l'IA");
                }
            }
        }
        // Raccourcis propres aux compétences : jamais de repli sur l'IA, et jamais
        // remplacés par un bip seul (`fast_path` reste faux).
        if let Some(shortcut) = self.skills.iter().find_map(|s| s.shortcut(text)) {
            debug!(route = "skill_shortcut", tool = %shortcut.tool);
            // Le droit d'abord : un invité entend le refus, pas « enregistrez votre voix ».
            // Testé comme si la voix était reconnue ; la confiance vient juste après.
            if let Some((_, spec)) = self.find(&shortcut.tool) {
                let as_sure = Speaker {
                    trust: Trust::Recognized,
                    ..source.who().clone()
                };
                if as_sure.check(&shortcut.tool, spec.risk) == Access::Denied {
                    return self.refuse(source.who(), &shortcut.tool).into();
                }
            }
            if !source.is_sure() {
                info!(tool = %shortcut.tool, "raccourci de compétence refusé : voix non reconnue");
                return Reply::failed(NEEDS_KNOWN_VOICE).into();
            }
            let intent = Intent::new(shortcut.tool, shortcut.args);
            return match self.run_or_ask(intent, source).await {
                Step::Answer(a) => a,
                Step::NotFound(m) => Reply::failed(m).into(),
            };
        }
        let mode = self.modes.get();
        let (backend, route) = match mode {
            Mode::Local => (&self.local, "local"),
            Mode::Online => (&self.online, "online"),
        };
        debug!(route, "compréhension par IA");
        let tools = self.tools();
        match backend.understand(text, &tools).await {
            Ok(Understanding::Tool(intent)) => {
                // Outil réservé aux raccourcis (agent de code) : jamais lancé par l'IA,
                // même si elle en invente le nom.
                let offered = tools.iter().any(|t| t.name == intent.tool)
                    || self.system.tools().iter().any(|t| t.name == intent.tool);
                if !offered && self.find(&intent.tool).is_some() {
                    warn!(tool = %intent.tool, "outil hors IA choisi par l'IA : refusé");
                    return Reply::failed(self.persona.addressed(UNKNOWN)).into();
                }
                if mode == Mode::Local
                    && intent.tool == "set_mode"
                    && intent.args.get("mode").and_then(|m| m.as_str()) == Some("online")
                {
                    return Reply::new(CONFIRM_ONLINE).into();
                }
                match self.run_or_ask(intent, source).await {
                    Step::Answer(a) => a,
                    Step::NotFound(m) => Reply::failed(m).into(),
                }
            }
            Ok(Understanding::Answer(a)) if a.trim().is_empty() => {
                Reply::failed(BAD_RESPONSE).into()
            }
            Ok(Understanding::Answer(a)) => Reply::new(a).into(),
            Err(e) => Reply::failed(llm_error_phrase(&e, mode)).into(),
        }
    }

    /// Comme [`Self::answer_fast`], sans l'attente de réponse.
    pub async fn handle_fast(&self, ear: &EarId, text: &str) -> Option<Reply> {
        self.answer_fast(ear, text).await.map(|a| a.reply)
    }

    /// Raccourcis seuls, sans jamais consulter l'IA (voix non vérifiée). `None` : aucun
    /// raccourci reconnu. Une action risquée n'est jamais proposée : `None` aussi. Une
    /// voix non vérifiée peut annuler, pas faire répéter : « répète » donne `None` (ni
    /// question reposée, donc pas de délai prolongé, ni dernière réponse redite).
    /// Les raccourcis propres aux compétences (`Skill::shortcut`) ne sont jamais
    /// consultés : l'agent de code, par exemple, exige une voix vérifiée.
    pub async fn answer_fast(&self, ear: &EarId, text: &str) -> Option<Answer> {
        let who = self.people.owner_speaker(Trust::Unverified);
        self.answer_fast_as(ear, text, &who).await
    }

    /// Comme [`Self::answer_fast`], pour `who` (en pratique `Unverified` : voix non
    /// vérifiée). Un outil que ses droits n'autorisent pas est refusé par une phrase.
    pub async fn answer_fast_as(&self, ear: &EarId, text: &str, who: &Speaker) -> Option<Answer> {
        let answer = match match_brain_command(text) {
            Some(BrainCommand::Repeat) => {
                info!("« répète » d'une voix non vérifiée : ignoré");
                return None;
            }
            Some(BrainCommand::Cancel) => self.cancel(),
            None => {
                self.clear_pending();
                let intent = match_fast(text)?;
                if self
                    .find(&intent.tool)
                    .is_some_and(|(_, t)| t.risk == Risk::Risky)
                {
                    info!(tool = %intent.tool, "action risquée refusée : voix non vérifiée");
                    return None;
                }
                info!(phrase = %text, "phrase reçue (raccourci seul)");
                let answer = match self.run_or_ask(intent, Source::Voice { ear, who }).await {
                    Step::Answer(a) => a,
                    Step::NotFound(m) => Reply::failed(m).into(),
                };
                Answer {
                    fast_path: true,
                    ..answer
                }
            }
        };
        self.remember(&answer);
        Some(answer)
    }

    /// Réponse à une question (`Listen { follow_up: true }`). `spoken_at` : début de
    /// la réponse ; `voice` : voix vérifiée ou non.
    ///
    /// Seul un « oui » d'une voix vérifiée, commencé dans le délai, exécute l'action en
    /// attente. Une voix vérifiée peut aussi faire répéter la question. Une voix non
    /// vérifiée ne peut qu'annuler : son « oui » (vérification stricte échouée) fait
    /// reposer la question une fois, puis annule ; son « répète » est ignoré. La
    /// question n'est reposée qu'une fois par action, toutes causes confondues ; le
    /// délai repart alors. Dans les autres cas, l'attente est effacée.
    ///
    /// Seule l'oreille qui a posé la question peut y répondre : la réponse d'une autre
    /// oreille ne donne rien, et l'attente est effacée par sécurité.
    pub async fn answer_follow_up(
        &self,
        ear: &EarId,
        text: &str,
        spoken_at: Instant,
        voice: ReplyVoice,
    ) -> Answer {
        let trust = match voice {
            ReplyVoice::Verified => Trust::Recognized,
            ReplyVoice::Unverified => Trust::Unverified,
        };
        let who = self.people.owner_speaker(trust);
        self.answer_follow_up_as(ear, text, spoken_at, voice, &who)
            .await
    }

    /// Comme [`Self::answer_follow_up`], pour `who`. Au « oui », ses droits sont
    /// revérifiés (`Speaker::may_confirm`) : rôle changé depuis la question, ou
    /// personne sans le droit `confirm` : refus, et l'attente est effacée.
    pub async fn answer_follow_up_as(
        &self,
        ear: &EarId,
        text: &str,
        spoken_at: Instant,
        voice: ReplyVoice,
        who: &Speaker,
    ) -> Answer {
        {
            let mut pending = self.pending.lock().unwrap();
            if let Some(p) = pending.as_ref()
                && &p.ear != ear
            {
                info!(
                    tool = %p.intent.tool,
                    attendue = %p.ear,
                    oreille = %ear,
                    "réponse d'une autre oreille : rien n'est fait"
                );
                *pending = None;
                return Reply::new("").into();
            }
        }
        let verified = voice == ReplyVoice::Verified;
        if match_brain_command(text) == Some(BrainCommand::Repeat) {
            if verified {
                return self.repeat(ear);
            }
            info!("« répète » d'une voix non vérifiée : ignoré");
            self.clear_pending();
            return Reply::new("").into();
        }
        let pending = self.pending.lock().unwrap().take();
        let reply = match (pending, match_confirmation(text)) {
            (None, Some(true)) => Reply::new(NOTHING_TO_CONFIRM),
            (None, _) => Reply::new(""),
            (Some(p), _) if spoken_at.saturating_duration_since(p.asked_at) > REPLY_WINDOW => {
                info!(tool = %p.intent.tool, "réponse trop tardive : rien n'est fait");
                Reply::new(TOO_LATE)
            }
            (Some(p), Some(true)) if verified => match self.find(&p.intent.tool) {
                Some((_, spec)) if !who.may_confirm(&p.intent.tool, spec.risk) => {
                    info!(
                        demandeur = %p.person,
                        personne = %who.person,
                        "confirmation sans droit"
                    );
                    return self.refuse_answer(who, &p.intent.tool);
                }
                Some((skill, _)) => {
                    info!(tool = %p.intent.tool, personne = %who.person, "action confirmée");
                    match self.execute(skill.as_ref(), p.intent).await {
                        Step::Answer(a) => a.reply,
                        Step::NotFound(m) => Reply::failed(m),
                    }
                }
                None => Reply::failed(self.persona.addressed(UNKNOWN)),
            },
            (Some(mut p), Some(true)) => {
                let tool = p.intent.tool.clone();
                if self.reask(&mut p) {
                    info!(tool = %tool, "« oui » d'une voix non vérifiée : question reposée");
                    *self.pending.lock().unwrap() = Some(p);
                    let answer = Answer {
                        reply: Reply::new(self.persona.addressed(VOICE_NOT_RECOGNIZED)),
                        awaits_reply: true,
                        fast_path: false,
                    };
                    self.remember(&answer);
                    return answer;
                }
                info!(tool = %tool, "« oui » d'une voix non vérifiée, deux fois : annulé");
                Reply::new(VOICE_REFUSED)
            }
            (Some(p), Some(false)) => {
                info!(tool = %p.intent.tool, "action annulée");
                Reply::new(CANCELLED)
            }
            // Fail-safe : une autre phrase efface l'attente, même d'une voix non vérifiée
            // (une télévision peut donc annuler, jamais confirmer).
            (Some(p), None) => {
                info!(tool = %p.intent.tool, "ni oui ni non : rien n'est fait");
                Reply::new(NOT_DONE)
            }
        };
        let answer = Answer::from(reply);
        self.remember(&answer);
        answer
    }

    /// Prépare une question reposée : une seule fois par action, le délai repart.
    /// Faux si elle l'a déjà été.
    fn reask(&self, p: &mut Pending) -> bool {
        if p.reasked {
            return false;
        }
        p.reasked = true;
        p.asked_at = self.clock.now();
        true
    }

    fn cancel(&self) -> Answer {
        if self.clear_pending() {
            info!("action en attente annulée");
            Reply::new(CANCELLED).into()
        } else {
            Reply::new(self.persona.addressed(NOTHING_TO_CANCEL)).into()
        }
    }

    /// Redit la question en attente de l'oreille `ear` (une seule fois par action ; le
    /// délai repart), sinon la dernière phrase. Rien n'est réexécuté. Voix vérifiée
    /// seulement. La question d'une autre oreille n'est ni reposée ni effacée ici.
    fn repeat(&self, ear: &EarId) -> Answer {
        let now = self.clock.now();
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.as_ref().is_some_and(|p| &p.ear != ear) {
                return self.repeat_last();
            }
            // Prise dans tous les cas : trop tardive, elle est effacée.
            if let Some(mut p) = pending.take()
                && now.saturating_duration_since(p.asked_at) <= REPLY_WINDOW
            {
                if !self.reask(&mut p) {
                    info!(tool = %p.intent.tool, "question déjà reposée : rien n'est fait");
                    return Reply::new(NO_MORE_REASK).into();
                }
                let question = p.question.clone();
                *pending = Some(p);
                return Answer {
                    reply: Reply::new(question),
                    awaits_reply: true,
                    fast_path: false,
                };
            }
        }
        self.repeat_last()
    }

    /// Redit la dernière phrase, sans toucher à l'action en attente.
    fn repeat_last(&self) -> Answer {
        match self.last_speech.lock().unwrap().clone() {
            Some(s) => Reply::new(s).into(),
            None => Reply::new(NOTHING_SAID).into(),
        }
    }

    fn remember(&self, answer: &Answer) {
        let speech = answer.reply.speech.trim();
        if !speech.is_empty() {
            *self.last_speech.lock().unwrap() = Some(speech.to_string());
        }
    }

    /// Compétence et description de l'outil `tool`.
    fn find(&self, tool: &str) -> Option<(Arc<dyn Skill>, ToolSpec)> {
        let system: Arc<dyn Skill> = self.system.clone();
        self.skills
            .iter()
            .cloned()
            .chain(std::iter::once(system))
            .find_map(|s| {
                let spec = s.tools().into_iter().find(|t| t.name == tool)?;
                Some((s, spec))
            })
    }

    /// Exécute l'intention, ou la met en attente d'un « oui » si l'outil est risqué.
    /// Texte tapé : un outil risqué est refusé, sans attente (le « oui » ne pourrait
    /// venir que d'une oreille, pour une demande qu'elle n'a pas entendue).
    async fn run_or_ask(&self, intent: Intent, source: Source<'_>) -> Step {
        let Some((skill, spec)) = self.find(&intent.tool) else {
            return Step::Answer(Reply::failed(self.persona.addressed(UNKNOWN)).into());
        };
        if source.who().check(&intent.tool, spec.risk) == Access::Denied {
            return Step::Answer(self.refuse(source.who(), &intent.tool).into());
        }
        if spec.risk == Risk::Risky {
            let Source::Voice { ear, who } = source else {
                info!(tool = %intent.tool, "action risquée tapée : refusée, à demander à voix haute");
                return Step::Answer(Reply::new(self.persona.addressed(ASK_ALOUD)).into());
            };
            let question = skill
                .confirmation(&intent.tool, &intent.args)
                .unwrap_or_else(|| format!("{} Je lance ?", spec.description.trim()));
            let question = self.persona.addressed(&question);
            info!(tool = %intent.tool, oreille = %ear, "action risquée : confirmation demandée");
            *self.pending.lock().unwrap() = Some(Pending {
                ear: ear.clone(),
                person: who.person.clone(),
                intent,
                question: question.clone(),
                asked_at: self.clock.now(),
                reasked: false,
            });
            return Step::Answer(Answer {
                reply: Reply::new(question),
                awaits_reply: true,
                fast_path: false,
            });
        }
        self.execute(skill.as_ref(), intent).await
    }

    /// Refus d'un outil que les droits de `who` n'autorisent pas.
    fn refuse(&self, who: &Speaker, tool: &str) -> Reply {
        info!(personne = %who.person, role = %who.role, outil = %tool, "droit refusé");
        Reply::failed(REFUSED)
    }

    fn refuse_answer(&self, who: &Speaker, tool: &str) -> Answer {
        let answer = Answer::from(self.refuse(who, tool));
        self.remember(&answer);
        answer
    }

    async fn execute(&self, skill: &dyn Skill, intent: Intent) -> Step {
        let tool = intent.tool.clone();
        let limit = skill.timeout(&tool).unwrap_or(self.skill_timeout);
        let call = skill.call(&intent.tool, intent.args);
        let result = match tokio::time::timeout(limit, call).await {
            Ok(r) => r,
            Err(_) => {
                warn!(tool = %intent.tool, "délai de la compétence dépassé");
                return Step::Answer(Reply::failed(TOO_SLOW).into());
            }
        };
        let reply = match result {
            // Réussite sans phrase de la compétence (médias, volume) : accusé bref.
            Ok(reply) if reply.outcome == Outcome::Done && reply.speech.trim().is_empty() => {
                Reply::done(DONE)
            }
            Ok(reply) => reply,
            Err(SkillError::NotFound(m)) => return Step::NotFound(m),
            Err(SkillError::UnknownTool(_) | SkillError::BadArgs(_)) => {
                Reply::failed(self.persona.addressed(UNKNOWN))
            }
            Err(SkillError::Failed(detail)) => {
                // Le détail est technique : journalisé, pas dit.
                warn!(tool = %tool, detail = %detail, "échec de la compétence");
                Reply::failed(self.persona.addressed(FAILED))
            }
        };
        Step::Answer(reply.into())
    }
}

const SKILL_TIMEOUT: Duration = Duration::from_secs(5);
/// Délai pour commencer à répondre à une question : les 8 s de l'oreille après la
/// question, plus le temps de la synthétiser et de la dire.
pub const REPLY_WINDOW: Duration = Duration::from_secs(8 + 4);
// Phrases fixes, brèves et polies. Celles marquées « adressée » reçoivent le nom
// d'adresse (`Persona::addressed`) ; les autres non, par parcimonie.
/// Accusé d'une réussite sans phrase propre.
const DONE: &str = "Bien.";
/// Adressée.
const UNKNOWN: &str = "Je crains de ne pas savoir faire cela.";
const BAD_RESPONSE: &str = "Pardon, la réponse de l'IA est incompréhensible.";
const TOO_SLOW: &str = "Désolé, la commande a pris trop de temps.";
/// Adressée.
const FAILED: &str = "Désolé, cela n'a pas fonctionné.";
const CONFIRM_ONLINE: &str = "Pour cela, dites « passe en mode en ligne ».";
const CANCELLED: &str = "Entendu, j'annule.";
/// Adressée.
const NOTHING_TO_CANCEL: &str = "Il n'y a rien à annuler.";
const NOTHING_TO_CONFIRM: &str = "Il n'y a rien à confirmer.";
const NOTHING_SAID: &str = "Je n'ai encore rien dit.";
const TOO_LATE: &str = "Le délai est passé, je ne fais rien.";
const NOT_DONE: &str = "Sans réponse claire, je ne fais rien.";
/// Adressée.
const VOICE_NOT_RECOGNIZED: &str =
    "Je n'ai pas bien reconnu votre voix. Dites « oui, vas-y » pour confirmer.";
const VOICE_REFUSED: &str = "Je ne reconnais toujours pas votre voix : je ne fais rien.";
/// Droit refusé (spec §5.8) : exacte, non adressée.
pub const REFUSED: &str = "Je ne peux pas faire ça pour vous.";
const NO_MORE_REASK: &str = "Je ne repose pas la question : je ne fais rien.";
/// Adressée.
const ASK_ALOUD: &str = "Pour cela, demandez-le-moi à voix haute.";
/// Raccourci de compétence (agent de code) demandé par une voix non reconnue.
pub const NEEDS_KNOWN_VOICE: &str = "Je ne confie une tâche à l'agent qu'à une voix reconnue : \
                                     enregistrez votre voix avec jarvis-ear enroll.";

const LOCAL_UNREACHABLE: &str = "L'IA locale ne répond pas. LM Studio est-il lancé ?";
const ONLINE_UNREACHABLE: &str =
    "L'IA en ligne est injoignable. Dites « passe en mode local » pour vous en passer.";
const LOCAL_REJECTED: &str = "L'IA locale refuse la demande. Le modèle est-il installé ?";

fn llm_error_phrase(e: &LlmError, mode: Mode) -> String {
    match (e, mode) {
        (LlmError::Unreachable(_), Mode::Local) => LOCAL_UNREACHABLE.into(),
        (LlmError::Unreachable(_), Mode::Online) => ONLINE_UNREACHABLE.into(),
        (LlmError::Rejected(_), Mode::Local) => LOCAL_REJECTED.into(),
        (LlmError::Rejected(_), Mode::Online) => "L'IA en ligne refuse la clé.".into(),
        (LlmError::MissingApiKey(var), _) => {
            format!("Il manque la clé de l'IA en ligne, à définir dans {var}.")
        }
        (LlmError::BadResponse(_), _) => BAD_RESPONSE.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeLlm, FakeSkill, tool_understanding};

    /// Oreille des tests (une seule).
    fn ear() -> EarId {
        EarId::new("test")
    }

    struct Setup {
        brain: Brain,
        local: Arc<FakeLlm>,
        online: Arc<FakeLlm>,
        skill: Arc<FakeSkill>,
        path: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    fn setup(local: FakeLlm, online: FakeLlm) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mode");
        let local = Arc::new(local);
        let online = Arc::new(online);
        let skill = Arc::new(FakeSkill::new(&["media_next"]));
        let brain = Brain::new(
            vec![skill.clone()],
            local.clone(),
            online.clone(),
            Arc::new(ModeStore::new(path.clone(), Mode::Local)),
        );
        Setup {
            brain,
            local,
            online,
            skill,
            path,
            _dir: dir,
        }
    }

    fn answer(s: &str) -> FakeLlm {
        FakeLlm::new(Ok(Understanding::Answer(s.into())))
    }

    #[tokio::test]
    async fn raccourci_sans_ia() {
        let s = setup(answer("x"), answer("y"));
        let r = s.brain.handle_text("suivant").await;
        assert_eq!(r.speech, "ok");
        assert_eq!(s.skill.calls()[0].0, "media_next");
        assert_eq!(s.local.calls() + s.online.calls(), 0);
    }

    #[tokio::test]
    async fn raccourci_seul_n_appelle_jamais_l_ia() {
        let s = setup(answer("x"), answer("y"));
        assert_eq!(
            s.brain.handle_fast(&ear(), "suivant").await.unwrap().speech,
            "ok"
        );
        assert!(
            s.brain
                .handle_fast(&ear(), "quel temps fait-il")
                .await
                .is_none()
        );
        // Cible introuvable : le message est dit, sans repli sur l'IA.
        let skill = FakeSkill::new(&["media_next"])
            .with_result(Err(SkillError::NotFound("Aucun lecteur.".into())));
        let brain = Brain::new(
            vec![Arc::new(skill)],
            s.local.clone(),
            s.online.clone(),
            Arc::new(ModeStore::new(s.path.clone(), Mode::Local)),
        );
        assert_eq!(
            brain.handle_fast(&ear(), "suivant").await.unwrap().speech,
            "Aucun lecteur."
        );
        assert_eq!(s.local.calls() + s.online.calls(), 0);
    }

    #[tokio::test]
    async fn mode_local_n_appelle_que_le_local() {
        let s = setup(answer("Il fait beau."), answer("y"));
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(r.speech, "Il fait beau.");
        assert_eq!((s.local.calls(), s.online.calls()), (1, 0));
    }

    #[tokio::test]
    async fn mode_en_ligne() {
        let s = setup(answer("x"), answer("Soleil."));
        s.brain.set_mode(Mode::Online).unwrap();
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(r.speech, "Soleil.");
        assert_eq!((s.local.calls(), s.online.calls()), (0, 1));
    }

    #[tokio::test]
    async fn switch_vocal_persiste() {
        let s = setup(answer("x"), answer("y"));
        let r = s.brain.handle_text("passe en mode en ligne").await;
        assert_eq!(r.speech, "Je passe en mode en ligne.");
        assert_eq!(
            ModeStore::new(s.path.clone(), Mode::Local).get(),
            Mode::Online
        );
    }

    #[tokio::test]
    async fn outil_inconnu_reponse_polie() {
        let s = setup(
            FakeLlm::new(Ok(tool_understanding("lance_fusee"))),
            answer("y"),
        );
        let r = s.brain.handle_text("lance une fusée").await;
        assert_eq!(r.speech, "Je crains de ne pas savoir faire cela, Monsieur.");
    }

    #[tokio::test]
    async fn serveur_local_injoignable() {
        let s = setup(
            FakeLlm::new(Err(LlmError::Unreachable("x".into()))),
            answer("y"),
        );
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(
            r.speech,
            "L'IA locale ne répond pas. LM Studio est-il lancé ?"
        );
        assert_eq!(s.brain.mode(), Mode::Local);
        assert_eq!(s.online.calls(), 0);
    }

    #[tokio::test]
    async fn refus_traduit_en_phrase() {
        let s = setup(
            FakeLlm::new(Err(LlmError::Rejected("404".into()))),
            FakeLlm::new(Err(LlmError::Rejected("401".into()))),
        );
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(
            r.speech,
            "L'IA locale refuse la demande. Le modèle est-il installé ?"
        );
        s.brain.set_mode(Mode::Online).unwrap();
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(r.speech, "L'IA en ligne refuse la clé.");
    }

    fn not_found_skill() -> Arc<FakeSkill> {
        Arc::new(
            FakeSkill::new(&["app_launch"])
                .with_result(Err(SkillError::NotFound("Je ne trouve pas x.".into()))),
        )
    }

    fn brain_with(skill: Arc<FakeSkill>, local: Arc<FakeLlm>) -> (Brain, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let brain = Brain::new(
            vec![skill],
            local,
            Arc::new(answer("y")),
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        );
        (brain, dir)
    }

    #[tokio::test]
    async fn raccourci_introuvable_repli_ia() {
        let skill = not_found_skill();
        let local = Arc::new(answer("Voici la réponse de l'IA."));
        let (brain, _dir) = brain_with(skill.clone(), local.clone());
        let r = brain.handle_text("ouvre le truc").await;
        assert_eq!(r.speech, "Voici la réponse de l'IA.");
        assert_eq!(local.calls(), 1);
        assert_eq!(skill.calls().len(), 1);
    }

    #[tokio::test]
    async fn ia_introuvable_dit_le_message() {
        let local = Arc::new(FakeLlm::new(Ok(tool_understanding("app_launch"))));
        let (brain, _dir) = brain_with(not_found_skill(), local);
        let r = brain.handle_text("fais apparaître le truc").await;
        assert_eq!(r.speech, "Je ne trouve pas x.");
    }

    #[tokio::test]
    async fn competence_trop_lente() {
        let skill =
            Arc::new(FakeSkill::new(&["media_next"]).with_delay(Duration::from_millis(500)));
        let (brain, _dir) = brain_with(skill, Arc::new(answer("x")));
        let brain = brain.with_skill_timeout(Duration::from_millis(30));
        let r = brain.handle_text("suivant").await;
        assert_eq!(r.speech, "Désolé, la commande a pris trop de temps.");
    }

    #[tokio::test(start_paused = true)]
    async fn delai_propre_a_la_competence() {
        // Délai propre de 10 s : un outil de 6 s répond.
        let lente = FakeSkill::new(&["media_next"]).with_delay(Duration::from_secs(6));
        let skill = Arc::new(lente.with_timeout(Duration::from_secs(10)));
        let (brain, _dir) = brain_with(skill, Arc::new(answer("x")));
        let r = brain.handle_text("suivant").await;
        assert_eq!(r.speech, "ok");
        // Sans délai propre : les 5 s communes, trop court.
        let skill = Arc::new(FakeSkill::new(&["media_next"]).with_delay(Duration::from_secs(6)));
        let (brain, _dir) = brain_with(skill, Arc::new(answer("x")));
        let r = brain.handle_text("suivant").await;
        assert_eq!(r.speech, "Désolé, la commande a pris trop de temps.");
    }

    #[test]
    fn outils_de_l_ia_sans_outils_systeme() {
        let s = setup(answer("x"), answer("y"));
        let names: Vec<String> = s.brain.tools().into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["media_next".to_string()]);
    }

    #[tokio::test]
    async fn les_outils_envoyes_a_l_ia_excluent_le_systeme() {
        let s = setup(answer("x"), answer("y"));
        s.brain.handle_text("quel temps fait-il").await;
        let sent = s.local.last_tools();
        assert_eq!(sent, vec!["media_next".to_string()]);
    }

    #[tokio::test]
    async fn ia_ne_bascule_pas_en_ligne() {
        let set_online = Understanding::Tool(Intent::new(
            "set_mode",
            serde_json::json!({"mode": "online"}),
        ));
        let s = setup(FakeLlm::new(Ok(set_online)), answer("y"));
        let r = s.brain.handle_text("passe sur l'IA en ligne").await;
        assert_eq!(r.speech, "Pour cela, dites « passe en mode en ligne ».");
        assert_eq!(s.brain.mode(), Mode::Local);
    }

    #[tokio::test]
    async fn en_ligne_injoignable_ne_bascule_pas() {
        let s = setup(
            answer("x"),
            FakeLlm::new(Err(LlmError::Unreachable("x".into()))),
        );
        s.brain.set_mode(Mode::Online).unwrap();
        let r = s.brain.handle_text("quel temps fait-il").await;
        assert_eq!(
            r.speech,
            "L'IA en ligne est injoignable. Dites « passe en mode local » pour vous en passer."
        );
        assert_eq!(s.brain.mode(), Mode::Online);
        assert_eq!(s.local.calls(), 0);
    }

    #[tokio::test]
    async fn cle_absente_reponse_vide_et_echec_competence() {
        let s = setup(
            FakeLlm::new(Err(LlmError::MissingApiKey("ANTHROPIC_API_KEY".into()))),
            answer("y"),
        );
        let r = s.brain.handle_text("bonjour").await;
        assert_eq!(
            r.speech,
            "Il manque la clé de l'IA en ligne, à définir dans ANTHROPIC_API_KEY."
        );

        let s = setup(answer("  "), answer("y"));
        let r = s.brain.handle_text("bonjour").await;
        assert_eq!(r.speech, "Pardon, la réponse de l'IA est incompréhensible.");

        let dir = tempfile::tempdir().unwrap();
        let skill = Arc::new(
            FakeSkill::new(&["media_next"]).with_result(Err(SkillError::Failed("boum".into()))),
        );
        let brain = Brain::new(
            vec![skill],
            Arc::new(answer("x")),
            Arc::new(answer("y")),
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        );
        assert_eq!(
            brain.handle_text("suivant").await.speech,
            "Désolé, cela n'a pas fonctionné, Monsieur."
        );
    }

    // --- Confirmations, « annule », « répète » ---

    use crate::testing::ManualClock;
    use jarvis_skills::FakeRunner;
    use jarvis_skills::power::{POWER_DELAY, PowerSkill};

    struct Power {
        brain: Brain,
        runner: Arc<FakeRunner>,
        clock: Arc<ManualClock>,
        skill: Arc<FakeSkill>,
        _dir: tempfile::TempDir,
    }

    fn power_with(llm: FakeLlm) -> Power {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::new());
        let clock = Arc::new(ManualClock::new());
        let skill = Arc::new(FakeSkill::new(&["media_next"]).with_result(Ok(Reply::done("Bien."))));
        let llm = Arc::new(llm);
        let brain = Brain::new(
            vec![skill.clone(), Arc::new(PowerSkill::new(runner.clone()))],
            llm.clone(),
            llm,
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        )
        .with_clock(clock.clone());
        Power {
            brain,
            runner,
            clock,
            skill,
            _dir: dir,
        }
    }

    fn power() -> Power {
        power_with(answer("x"))
    }

    fn suspend_call() -> Vec<(String, Vec<String>)> {
        vec![("systemctl".to_string(), vec!["suspend".to_string()])]
    }

    impl Power {
        async fn ask(&self) {
            let a = self.brain.answer_text(&ear(), "mets le PC en veille").await;
            assert_eq!(a.reply.speech, "Je mets le PC en veille, Monsieur ?");
            assert!(a.awaits_reply);
        }

        async fn reply(&self, text: &str) -> Answer {
            self.reply_as(text, ReplyVoice::Verified).await
        }

        async fn reply_as(&self, text: &str, voice: ReplyVoice) -> Answer {
            self.brain
                .answer_follow_up(&ear(), text, self.brain.now(), voice)
                .await
        }

        /// Commandes lancées, une fois passé le délai de l'alimentation (horloge de
        /// tokio en pause : l'attente est instantanée).
        async fn ran(&self) -> Vec<(String, Vec<String>)> {
            tokio::time::sleep(POWER_DELAY + Duration::from_secs(1)).await;
            self.runner.calls()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn action_risquee_attend_confirmation() {
        let p = power();
        p.ask().await;
        assert!(p.ran().await.is_empty());
        assert!(p.brain.has_pending());
        // Par l'IA aussi.
        let p = power_with(FakeLlm::new(Ok(tool_understanding("shutdown"))));
        let a = p.brain.answer_text(&ear(), "coupe tout, j'ai fini").await;
        assert_eq!(a.reply.speech, "J'éteins le PC, Monsieur ?");
        assert!(a.awaits_reply);
        assert!(p.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn oui_execute() {
        let p = power();
        p.ask().await;
        p.clock.advance(Duration::from_secs(5));
        let a = p.reply("Oui, vas-y.").await;
        assert_eq!(a.reply.outcome, Outcome::Done);
        assert!(!a.awaits_reply);
        assert_eq!(p.ran().await, suspend_call());
        assert!(!p.brain.has_pending());
        // Une seule fois : un second « oui » ne refait rien.
        let a = p.reply("oui").await;
        assert_eq!(a.reply.speech, "Il n'y a rien à confirmer.");
        assert_eq!(p.ran().await.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn non_annule() {
        let p = power();
        p.ask().await;
        let a = p.reply("non").await;
        assert_eq!(a.reply, Reply::new("Entendu, j'annule."));
        assert!(p.ran().await.is_empty());
        assert!(!p.brain.has_pending());
    }

    #[tokio::test(start_paused = true)]
    async fn delai_depasse_rien() {
        let p = power();
        p.ask().await;
        p.clock.advance(REPLY_WINDOW + Duration::from_millis(1));
        let a = p.reply("oui").await;
        assert_eq!(a.reply, Reply::new("Le délai est passé, je ne fais rien."));
        assert!(p.ran().await.is_empty());
        assert!(!p.brain.has_pending());
    }

    #[tokio::test(start_paused = true)]
    async fn debut_de_reponse_dans_le_delai_accepte() {
        // Réponse arrivée après le délai, mais commencée avant : acceptée.
        let p = power();
        p.ask().await;
        p.clock.advance(REPLY_WINDOW + Duration::from_secs(1));
        let started = p.brain.now() - Duration::from_secs(2);
        p.brain
            .answer_follow_up(&ear(), "oui", started, ReplyVoice::Verified)
            .await;
        assert_eq!(p.ran().await, suspend_call());
    }

    #[tokio::test(start_paused = true)]
    async fn autre_phrase_annule_l_attente() {
        let p = power();
        p.ask().await;
        let a = p.reply("mets pause").await;
        assert_eq!(a.reply, Reply::new("Sans réponse claire, je ne fais rien."));
        assert!(!p.brain.has_pending());
        p.reply("oui").await;
        assert!(p.ran().await.is_empty());
        assert!(p.skill.calls().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn nouvelle_commande_efface_l_attente() {
        let p = power();
        p.ask().await;
        p.brain.answer_text(&ear(), "suivant").await;
        assert!(!p.brain.has_pending());
        p.reply("oui").await;
        assert!(p.ran().await.is_empty());
        // Une question libre (IA) aussi.
        p.ask().await;
        p.brain.answer_text(&ear(), "quel temps fait-il").await;
        assert!(!p.brain.has_pending());
        // Et une nouvelle action risquée remplace la première.
        p.ask().await;
        p.brain.answer_text(&ear(), "redémarre le PC").await;
        p.reply("oui").await;
        assert_eq!(
            p.ran().await,
            vec![("systemctl".to_string(), vec!["reboot".to_string()])]
        );
    }

    fn salon() -> EarId {
        EarId::new("salon")
    }

    fn cuisine() -> EarId {
        EarId::new("cuisine")
    }

    impl Power {
        /// Question posée par l'oreille `ear`.
        async fn ask_on(&self, ear: &EarId) {
            let a = self.brain.answer_text(ear, "mets le PC en veille").await;
            assert!(a.awaits_reply);
        }

        async fn reply_on(&self, ear: &EarId, text: &str) -> Answer {
            self.brain
                .answer_follow_up(ear, text, self.brain.now(), ReplyVoice::Verified)
                .await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn confirmation_sur_la_meme_oreille_execute() {
        let p = power();
        p.ask_on(&salon()).await;
        let a = p.reply_on(&salon(), "oui").await;
        assert_eq!(a.reply.outcome, Outcome::Done);
        assert_eq!(p.ran().await, suspend_call());
        // « non » sur la même oreille : annulé.
        p.ask_on(&salon()).await;
        let a = p.reply_on(&salon(), "non").await;
        assert_eq!(a.reply, Reply::new("Entendu, j'annule."));
    }

    #[tokio::test(start_paused = true)]
    async fn oui_d_une_autre_oreille_n_execute_pas() {
        let p = power();
        p.ask_on(&salon()).await;
        let a = p.reply_on(&cuisine(), "oui").await;
        assert_eq!(a.reply, Reply::new(""));
        assert!(!a.awaits_reply);
        // Par sécurité, l'attente est effacée : le « oui » du salon ne fait plus rien.
        assert!(!p.brain.has_pending());
        let a = p.reply_on(&salon(), "oui").await;
        assert_eq!(a.reply.speech, "Il n'y a rien à confirmer.");
        assert!(p.ran().await.is_empty());
        // Ni « répète » : la question n'est pas reposée ailleurs.
        p.ask_on(&salon()).await;
        let a = p.reply_on(&cuisine(), "répète").await;
        assert_eq!(a.reply, Reply::new(""));
        assert!(!p.brain.has_pending());
    }

    #[tokio::test(start_paused = true)]
    async fn commande_d_une_autre_oreille_annule_l_attente() {
        let p = power();
        p.ask_on(&salon()).await;
        p.brain.answer_text(&cuisine(), "suivant").await;
        assert!(!p.brain.has_pending());
        p.reply_on(&salon(), "oui").await;
        assert!(p.ran().await.is_empty());
        // « annule » d'une autre oreille aussi, même d'une voix non vérifiée.
        p.ask_on(&salon()).await;
        let a = p.brain.answer_fast(&cuisine(), "annule").await.unwrap();
        assert_eq!(a.reply, Reply::new("Entendu, j'annule."));
        assert!(!p.brain.has_pending());
        // Une action risquée d'une autre oreille se confirme sur celle-ci.
        p.ask_on(&salon()).await;
        p.ask_on(&cuisine()).await;
        p.reply_on(&salon(), "oui").await;
        assert!(p.ran().await.is_empty());
        p.ask_on(&cuisine()).await;
        p.reply_on(&cuisine(), "oui").await;
        assert_eq!(p.ran().await, suspend_call());
    }

    #[tokio::test(start_paused = true)]
    async fn repete_d_une_autre_oreille_ne_repose_pas_la_question() {
        let p = power();
        p.brain.answer_text(&salon(), "suivant").await;
        p.ask_on(&salon()).await;
        let a = p.brain.answer_text(&cuisine(), "répète").await;
        // La dernière phrase, sans attente de réponse ni question reposée.
        assert_eq!(a.reply, Reply::new("Je mets le PC en veille, Monsieur ?"));
        assert!(!a.awaits_reply);
        // Le salon garde sa question reposable (budget intact).
        let a = p.brain.answer_text(&salon(), "répète").await;
        assert_eq!(a.reply.speech, "Je mets le PC en veille, Monsieur ?");
        assert!(a.awaits_reply);
        p.reply_on(&salon(), "oui").await;
        assert_eq!(p.ran().await, suspend_call());
    }

    #[tokio::test(start_paused = true)]
    async fn annule_sans_attente() {
        let p = power();
        let a = p.brain.answer_text(&ear(), "annule").await;
        assert_eq!(a.reply, Reply::new("Il n'y a rien à annuler, Monsieur."));
        p.ask().await;
        let a = p.brain.answer_text(&ear(), "Jarvis, annule.").await;
        assert_eq!(a.reply, Reply::new("Entendu, j'annule."));
        assert!(!p.brain.has_pending());
        p.reply("oui").await;
        assert!(p.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn repete_la_derniere_reponse() {
        let p = power();
        let a = p.brain.answer_text(&ear(), "répète").await;
        assert_eq!(a.reply, Reply::new("Je n'ai encore rien dit."));
        p.brain.answer_text(&ear(), "suivant").await;
        let a = p.brain.answer_text(&ear(), "tu peux répéter").await;
        // Redit sans réexécuter, et sans bip de réussite.
        assert_eq!(a.reply, Reply::new("Bien."));
        assert_eq!(p.skill.calls().len(), 1);
        // Question en attente : elle est reposée, le délai repart.
        p.ask().await;
        p.clock.advance(Duration::from_secs(10));
        let a = p.brain.answer_text(&ear(), "répète").await;
        assert_eq!(a.reply.speech, "Je mets le PC en veille, Monsieur ?");
        assert!(a.awaits_reply);
        p.clock.advance(Duration::from_secs(10));
        p.reply("oui").await;
        assert_eq!(p.ran().await, suspend_call());
    }

    #[tokio::test(start_paused = true)]
    async fn nom_d_adresse_configurable() {
        let p = power();
        let brain = p.brain.with_persona(Persona::new("Charles"));
        let a = brain.answer_text(&ear(), "annule").await;
        assert_eq!(a.reply.speech, "Il n'y a rien à annuler, Charles.");
        let a = brain.answer_text(&ear(), "mets le PC en veille").await;
        assert_eq!(a.reply.speech, "Je mets le PC en veille, Charles ?");
        // Avec parcimonie : pas dans toutes les phrases.
        let a = brain
            .answer_follow_up(&ear(), "non", brain.now(), ReplyVoice::Verified)
            .await;
        assert_eq!(a.reply.speech, "Entendu, j'annule.");
        // Nom vide : jamais nommé.
        let p = power();
        let brain = p.brain.with_persona(Persona::new(""));
        let a = brain.answer_text(&ear(), "annule").await;
        assert_eq!(a.reply.speech, "Il n'y a rien à annuler.");
    }

    #[tokio::test(start_paused = true)]
    async fn voix_non_verifiee_sans_action_risquee() {
        let p = power();
        assert!(
            p.brain
                .answer_fast(&ear(), "mets le PC en veille")
                .await
                .is_none()
        );
        assert!(!p.brain.has_pending());
        // « annule » reste permis ; « répète » est ignoré (ni question reposée, ni
        // dernière réponse redite).
        assert!(p.brain.answer_fast(&ear(), "annule").await.is_some());
        assert!(p.brain.answer_fast(&ear(), "répète").await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn voix_non_verifiee_ne_prolonge_pas_l_attente() {
        let p = power();
        p.ask().await;
        p.clock.advance(Duration::from_secs(10));
        // « Jarvis, répète » court, non vérifié : ignoré, le délai ne repart pas.
        assert!(p.brain.answer_fast(&ear(), "répète").await.is_none());
        p.clock.advance(Duration::from_secs(5));
        let a = p.reply("oui").await;
        assert_eq!(a.reply, Reply::new("Le délai est passé, je ne fais rien."));
        assert!(p.ran().await.is_empty());
        // En réponse à la question : ignoré aussi, et l'attente est effacée.
        p.ask().await;
        let a = p.reply_as("répète", ReplyVoice::Unverified).await;
        assert_eq!(a.reply.speech, "");
        assert!(!a.awaits_reply);
        assert!(!p.brain.has_pending());
    }

    #[tokio::test(start_paused = true)]
    async fn voix_non_verifiee_ne_repete_pas() {
        let p = power();
        p.brain.answer_text(&ear(), "suivant").await;
        assert!(
            p.brain
                .answer_fast(&ear(), "tu peux répéter")
                .await
                .is_none()
        );
        let a = p.reply_as("répète", ReplyVoice::Unverified).await;
        assert_eq!(a.reply.speech, "");
    }

    #[tokio::test(start_paused = true)]
    async fn oui_non_verifie_redemande_une_fois_puis_annule() {
        let p = power();
        p.ask().await;
        p.clock.advance(Duration::from_secs(10));
        let a = p.reply_as("oui", ReplyVoice::Unverified).await;
        assert_eq!(
            a.reply.speech,
            "Je n'ai pas bien reconnu votre voix. Dites « oui, vas-y » pour confirmer, Monsieur."
        );
        assert!(a.awaits_reply);
        assert!(p.brain.has_pending());
        // Le délai repart de la nouvelle question.
        p.clock.advance(Duration::from_secs(10));
        let a = p.reply_as("oui", ReplyVoice::Unverified).await;
        assert_eq!(
            a.reply.speech,
            "Je ne reconnais toujours pas votre voix : je ne fais rien."
        );
        assert!(!a.awaits_reply);
        assert!(!p.brain.has_pending());
        assert!(p.ran().await.is_empty());
        // Sinon, la voix de Charles confirme après la nouvelle question.
        p.ask().await;
        p.reply_as("oui", ReplyVoice::Unverified).await;
        p.clock.advance(Duration::from_secs(10));
        p.reply("oui, vas-y").await;
        assert_eq!(p.ran().await, suspend_call());
    }

    #[tokio::test(start_paused = true)]
    async fn non_verifie_annule_seulement() {
        let p = power();
        p.ask().await;
        let a = p.reply_as("non", ReplyVoice::Unverified).await;
        assert_eq!(a.reply, Reply::new("Entendu, j'annule."));
        assert!(!p.brain.has_pending());
        // Une autre phrase non vérifiée efface aussi l'attente (sûr, quitte à ce qu'une
        // télévision annule).
        p.ask().await;
        let a = p.reply_as("mets pause", ReplyVoice::Unverified).await;
        assert_eq!(a.reply, Reply::new("Sans réponse claire, je ne fais rien."));
        assert!(!p.brain.has_pending());
        assert!(p.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn repete_pendant_la_question_redemande() {
        let p = power();
        p.ask().await;
        p.clock.advance(Duration::from_secs(10));
        let a = p.reply("répète").await;
        assert_eq!(a.reply.speech, "Je mets le PC en veille, Monsieur ?");
        assert!(a.awaits_reply);
        p.clock.advance(Duration::from_secs(10));
        p.reply("oui").await;
        assert_eq!(p.ran().await, suspend_call());
    }

    #[tokio::test(start_paused = true)]
    async fn une_seule_question_reposee_par_action() {
        let refus = Reply::new("Je ne repose pas la question : je ne fais rien.");
        // Deux « répète » de suite.
        let p = power();
        p.ask().await;
        assert!(p.reply("répète").await.awaits_reply);
        let a = p.reply("répète").await;
        assert_eq!(a.reply, refus);
        assert!(!a.awaits_reply);
        assert!(!p.brain.has_pending());
        // Avec le mot d'éveil aussi.
        p.ask().await;
        assert!(p.brain.answer_text(&ear(), "répète").await.awaits_reply);
        assert_eq!(p.brain.answer_text(&ear(), "répète").await.reply, refus);
        assert!(!p.brain.has_pending());
        // Le budget est partagé avec la vérification stricte d'un « oui » court.
        p.ask().await;
        assert!(p.reply_as("oui", ReplyVoice::Unverified).await.awaits_reply);
        assert_eq!(p.reply("répète").await.reply, refus);
        assert!(!p.brain.has_pending());
        assert!(p.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn say_ne_cree_pas_d_attente() {
        let p = power();
        let r = p.brain.handle_text("mets le PC en veille").await;
        assert_eq!(
            r.speech,
            "Pour cela, demandez-le-moi à voix haute, Monsieur."
        );
        assert!(!p.brain.has_pending());
        let a = p.reply("oui").await;
        assert_eq!(a.reply.speech, "Il n'y a rien à confirmer.");
        // Par l'IA aussi.
        let p = power_with(FakeLlm::new(Ok(tool_understanding("shutdown"))));
        p.brain.handle_text("coupe tout, j'ai fini").await;
        assert!(!p.brain.has_pending());
        // « répète » tapé ne prolonge pas une question posée à voix haute.
        let p = power();
        p.ask().await;
        p.clock.advance(Duration::from_secs(10));
        p.brain.handle_text("répète").await;
        p.clock.advance(Duration::from_secs(5));
        let a = p.reply("oui").await;
        assert_eq!(a.reply, Reply::new("Le délai est passé, je ne fais rien."));
        assert!(p.ran().await.is_empty());
    }

    #[tokio::test]
    async fn echecs_du_cerveau_avec_bip_d_echec() {
        let failed = |speech: &str| (speech.to_string(), Outcome::Failed);
        let got = |r: Reply| (r.speech, r.outcome);

        let s = setup(
            FakeLlm::new(Ok(tool_understanding("lance_fusee"))),
            answer("y"),
        );
        assert_eq!(
            got(s.brain.handle_text("lance une fusée").await),
            failed("Je crains de ne pas savoir faire cela, Monsieur.")
        );
        let s = setup(
            FakeLlm::new(Err(LlmError::Unreachable("x".into()))),
            answer("y"),
        );
        assert_eq!(
            got(s.brain.handle_text("quel temps").await),
            failed("L'IA locale ne répond pas. LM Studio est-il lancé ?")
        );
        let s = setup(
            FakeLlm::new(Err(LlmError::Rejected("x".into()))),
            answer("y"),
        );
        assert_eq!(
            s.brain.handle_text("quel temps").await.outcome,
            Outcome::Failed
        );
        let skill =
            Arc::new(FakeSkill::new(&["media_next"]).with_delay(Duration::from_millis(500)));
        let (brain, _dir) = brain_with(skill, Arc::new(answer("x")));
        let brain = brain.with_skill_timeout(Duration::from_millis(30));
        assert_eq!(brain.handle_text("suivant").await.outcome, Outcome::Failed);
        let local = Arc::new(FakeLlm::new(Ok(tool_understanding("app_launch"))));
        let (brain, _dir) = brain_with(not_found_skill(), local);
        assert_eq!(
            got(brain.handle_text("fais apparaître le truc").await),
            failed("Je ne trouve pas x.")
        );
        // Une réponse de l'IA reste une information.
        let s = setup(answer("Il fait beau."), answer("y"));
        assert_eq!(
            s.brain.handle_text("quel temps").await.outcome,
            Outcome::Info
        );
    }

    // --- Raccourcis propres aux compétences (agent de code) ---

    const PHRASE_AGENT: &str = "Jarvis, finis les tests du projet Jarvis";

    struct Agent {
        brain: Brain,
        llm: Arc<FakeLlm>,
        media: Arc<FakeSkill>,
        agent: Arc<FakeSkill>,
        _dir: tempfile::TempDir,
    }

    /// Médias (proposés à l'IA, réussite sans phrase) et l'agent factice `agent`.
    fn agent_with(agent: FakeSkill, persona: Persona) -> Agent {
        let dir = tempfile::tempdir().unwrap();
        let llm = Arc::new(answer("Réponse de l'IA."));
        let media = Arc::new(FakeSkill::new(&["media_next"]).with_result(Ok(Reply::done(""))));
        let agent = Arc::new(agent);
        let brain = Brain::new(
            vec![media.clone(), agent.clone()],
            llm.clone(),
            llm.clone(),
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        )
        .with_persona(persona);
        Agent {
            brain,
            llm,
            media,
            agent,
            _dir: dir,
        }
    }

    fn fake_agent() -> FakeSkill {
        FakeSkill::new(&["agent_start"])
            .with_shortcut(
                PHRASE_AGENT,
                "agent_start",
                serde_json::json!({"project": "Jarvis"}),
            )
            .with_llm_tools(false)
    }

    fn agent() -> Agent {
        agent_with(fake_agent(), Persona::default())
    }

    #[tokio::test]
    async fn raccourci_de_competence_apres_les_raccourcis() {
        // « suivant » est aussi un raccourci de l'agent : celui du cerveau passe avant.
        let skill = FakeSkill::new(&["agent_start"])
            .with_shortcut("suivant", "agent_start", serde_json::json!({}))
            .with_llm_tools(false);
        let a = agent_with(skill, Persona::default());
        assert_eq!(a.brain.handle_text("suivant").await.speech, "Bien.");
        assert_eq!(a.media.calls().len(), 1);
        assert!(a.agent.calls().is_empty());

        let a = agent();
        let r = a.brain.handle_text(PHRASE_AGENT).await;
        assert_eq!(r.speech, "ok");
        assert_eq!(
            a.agent.calls(),
            vec![(
                "agent_start".to_string(),
                serde_json::json!({"project": "Jarvis"})
            )]
        );
        assert_eq!(a.llm.calls(), 0);
        // Une autre phrase va à l'IA, comme avant.
        a.brain.handle_text("quel temps fait-il").await;
        assert_eq!(a.llm.calls(), 1);
        assert_eq!(a.agent.calls().len(), 1);
    }

    #[tokio::test]
    async fn raccourci_de_competence_introuvable_sans_ia() {
        let message = "Je ne connais pas le projet « Zorg ». Projets connus : Jarvis.";
        let skill = fake_agent().with_result(Err(SkillError::NotFound(message.into())));
        let a = agent_with(skill, Persona::default());
        let r = a.brain.handle_text(PHRASE_AGENT).await;
        assert_eq!((r.speech.as_str(), r.outcome), (message, Outcome::Failed));
        assert_eq!(a.llm.calls(), 0);
        assert_eq!(a.agent.calls().len(), 1);
    }

    #[tokio::test]
    async fn voix_non_verifiee_ignore_les_raccourcis_de_competence() {
        let a = agent();
        assert!(a.brain.answer_fast(&ear(), PHRASE_AGENT).await.is_none());
        assert!(a.brain.handle_fast(&ear(), PHRASE_AGENT).await.is_none());
        assert!(a.agent.calls().is_empty());
        assert_eq!(a.llm.calls(), 0);
        // La voix vérifiée, elle, lance l'outil.
        a.brain.answer_text(&ear(), PHRASE_AGENT).await;
        assert_eq!(a.agent.calls().len(), 1);
    }

    /// Toujours la même empreinte : toute voix est celle de Charles.
    struct SameVoice;

    impl crate::speaker::SpeakerVerifier for SameVoice {
        fn embed(&self, _audio: &[f32]) -> anyhow::Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }
    }

    fn known_voice(dir: &std::path::Path) -> Arc<crate::speaker::SpeakerGate> {
        let gate = crate::speaker::SpeakerGate::new(
            Arc::new(SameVoice),
            dir.join("charles.json"),
            0.55,
            0.65,
        );
        gate.enroll(&[vec![0.0; 16_000]]).unwrap();
        Arc::new(gate)
    }

    #[tokio::test]
    async fn voix_non_reconnue_refuse_les_raccourcis_de_competence() {
        let a = agent();
        let r = a.brain.answer_voice(&ear(), PHRASE_AGENT, false).await;
        assert_eq!(
            (r.reply.speech.as_str(), r.reply.outcome),
            (NEEDS_KNOWN_VOICE, Outcome::Failed)
        );
        assert!(!r.fast_path);
        assert!(a.agent.calls().is_empty());
        assert_eq!(a.llm.calls(), 0, "pas de repli sur l'IA");
        // Raccourcis du cerveau et IA : comme avant.
        assert_eq!(
            a.brain
                .answer_voice(&ear(), "suivant", false)
                .await
                .reply
                .speech,
            "Bien."
        );
        assert_eq!(a.media.calls().len(), 1);
        a.brain
            .answer_voice(&ear(), "quel temps fait-il", false)
            .await;
        assert_eq!(a.llm.calls(), 1);
        // Voix reconnue : l'outil part.
        a.brain.answer_voice(&ear(), PHRASE_AGENT, true).await;
        assert_eq!(a.agent.calls().len(), 1);
    }

    #[tokio::test]
    async fn oreille_sans_voix_reconnue_ne_lance_pas_l_agent() {
        use crate::listen::ListenService;
        use crate::speaker::SpeakerState;
        use crate::testing::{FakeDucker, FakeTranscriber};

        let voices = tempfile::tempdir().unwrap();
        let states = [
            SpeakerState::Disabled,
            SpeakerState::Unavailable { enrolled: false },
            // Vérification prête, mais aucune empreinte enregistrée.
            SpeakerState::Ready(Arc::new(crate::speaker::SpeakerGate::new(
                Arc::new(SameVoice),
                voices.path().join("absente.json"),
                0.55,
                0.65,
            ))),
        ];
        for state in states {
            let a = agent();
            let agent = a.agent.clone();
            let svc = ListenService::new(
                Arc::new(a.brain),
                Arc::new(FakeTranscriber::new(PHRASE_AGENT)),
                Arc::new(FakeDucker::new()),
            )
            .with_speaker_state(state);
            let r = svc.listen(&ear(), &[0; 3 * 16_000], false, false).await;
            let jarvis_protocol::ControlResponse::Heard { speech, .. } = r else {
                panic!("{r:?}");
            };
            assert_eq!(speech, NEEDS_KNOWN_VOICE);
            assert!(agent.calls().is_empty());
        }
        // Voix reconnue par son empreinte : l'agent part.
        let a = agent();
        let agent = a.agent.clone();
        let svc = ListenService::new(
            Arc::new(a.brain),
            Arc::new(FakeTranscriber::new(PHRASE_AGENT)),
            Arc::new(FakeDucker::new()),
        )
        .with_speaker(Some(known_voice(voices.path())));
        svc.listen(&ear(), &[0; 3 * 16_000], false, false).await;
        assert_eq!(agent.calls().len(), 1);
    }

    #[tokio::test]
    async fn outils_hors_ia_non_proposes() {
        let a = agent();
        let names: Vec<String> = a.brain.tools().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["media_next"]);
        a.brain.handle_text("quel temps fait-il").await;
        assert_eq!(a.llm.last_tools(), ["media_next"]);
        // L'IA ne voit pas l'outil, mais le raccourci le lance.
        a.brain.handle_text(PHRASE_AGENT).await;
        assert_eq!(a.agent.calls().len(), 1);
    }

    #[tokio::test]
    async fn raccourci_de_competence_jamais_en_bip_seul() {
        use crate::listen::ListenService;
        use crate::testing::{FakeDucker, FakeTranscriber};
        use jarvis_protocol::{ControlResponse, Sound};

        let depart = "Bien. Je confie cela à Claude.";
        let persona = Persona {
            sound_only_for_success: true,
            ..Persona::default()
        };
        let skill = fake_agent().with_result(Ok(Reply::done(depart)));
        let a = agent_with(skill, persona.clone());
        let r = a.brain.answer_text(&ear(), PHRASE_AGENT).await;
        assert!(!r.fast_path);
        assert_eq!(
            (r.reply.speech.as_str(), r.reply.outcome),
            (depart, Outcome::Done)
        );

        // Par l'oreille (voix reconnue), avec le bip seul activé : la phrase est dite
        // quand même.
        let skill = fake_agent().with_result(Ok(Reply::done(depart)));
        let a = agent_with(skill, persona);
        let voices = tempfile::tempdir().unwrap();
        let svc = ListenService::new(
            Arc::new(a.brain),
            Arc::new(FakeTranscriber::new(PHRASE_AGENT)),
            Arc::new(FakeDucker::new()),
        )
        .with_speaker(Some(known_voice(voices.path())));
        assert_eq!(
            svc.listen(&ear(), &[0; 3 * 16_000], false, false).await,
            ControlResponse::Heard {
                text: PHRASE_AGENT.into(),
                speech: depart.into(),
                ear_action: None,
                audio: None,
                sound: Some(Sound::Success),
            }
        );
    }
}

/// Garde-fou d'outil : les droits de qui parle (phase 8).
#[cfg(test)]
mod droits {
    use super::*;
    use crate::testing::{FakeLlm, FakeSkill, ManualClock, tool_understanding};
    use jarvis_skills::FakeRunner;
    use jarvis_skills::power::{POWER_DELAY, PowerSkill};

    const PHRASE_AGENT: &str = "Jarvis, finis les tests du projet Jarvis";
    const VEILLE: &str = "mets le PC en veille";

    struct Montage {
        brain: Brain,
        people: Arc<People>,
        llm: Arc<FakeLlm>,
        media: Arc<FakeSkill>,
        agent: Arc<FakeSkill>,
        runner: Arc<FakeRunner>,
        _dir: tempfile::TempDir,
    }

    fn montage_avec(llm: FakeLlm, people: Arc<People>) -> Montage {
        let dir = tempfile::tempdir().unwrap();
        let llm = Arc::new(llm);
        let media = Arc::new(
            FakeSkill::new(&["media_next", "app_launch", "web_search"])
                .with_result(Ok(Reply::done("Bien."))),
        );
        let agent = Arc::new(
            FakeSkill::new(&["agent_start"])
                .with_shortcut(
                    PHRASE_AGENT,
                    "agent_start",
                    serde_json::json!({"project": "Jarvis"}),
                )
                .with_llm_tools(false),
        );
        let runner = Arc::new(FakeRunner::new());
        let brain = Brain::new(
            vec![
                media.clone(),
                agent.clone(),
                Arc::new(PowerSkill::new(runner.clone())),
            ],
            llm.clone(),
            llm.clone(),
            Arc::new(ModeStore::new(dir.path().join("mode"), Mode::Local)),
        )
        .with_clock(Arc::new(ManualClock::new()))
        .with_people(people.clone());
        Montage {
            brain,
            people,
            llm,
            media,
            agent,
            runner,
            _dir: dir,
        }
    }

    fn montage(llm: FakeLlm) -> Montage {
        montage_avec(llm, Arc::new(People::admin_only("Charles")))
    }

    fn bavard() -> FakeLlm {
        FakeLlm::new(Ok(crate::llm::Understanding::Answer("Réponse.".into())))
    }

    impl Montage {
        fn tools(&self) -> Vec<String> {
            self.brain.tool_names()
        }

        /// Rôle `famille` (musique et applications) et Léa.
        fn avec_lea(&self) {
            self.role("famille", &["media_*", "app_launch"], false, false);
            self.people.add_person("Léa", "famille").unwrap();
        }

        fn role(&self, nom: &str, outils: &[&str], confirm: bool, agent: bool) {
            let outils = outils.iter().map(|s| s.to_string()).collect();
            self.people
                .add_role(nom, outils, confirm, agent, &self.tools())
                .unwrap();
        }

        fn lea(&self, trust: Trust) -> Speaker {
            self.people.speaker("Léa", trust)
        }

        async fn ran(&self) -> Vec<(String, Vec<String>)> {
            tokio::time::sleep(POWER_DELAY + Duration::from_secs(1)).await;
            self.runner.calls()
        }
    }

    fn ear() -> EarId {
        EarId::new("test")
    }

    fn refus(a: &Answer) {
        assert_eq!(
            (a.reply.speech.as_str(), a.reply.outcome),
            ("Je ne peux pas faire ça pour vous.", Outcome::Failed)
        );
        assert!(!a.awaits_reply);
    }

    #[tokio::test(start_paused = true)]
    async fn invite_peut_lancer_une_musique() {
        let m = montage(bavard());
        m.avec_lea();
        let a = m
            .brain
            .answer_as(&ear(), "suivant", &m.lea(Trust::Unverified))
            .await;
        assert_eq!(
            (a.reply.speech.as_str(), a.reply.outcome),
            ("Bien.", Outcome::Done)
        );
        assert_eq!(m.media.calls().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn invite_ne_peut_pas_mettre_en_veille() {
        let m = montage(bavard());
        m.avec_lea();
        for trust in [Trust::Unverified, Trust::Accepted, Trust::Recognized] {
            let a = m.brain.answer_as(&ear(), VEILLE, &m.lea(trust)).await;
            refus(&a);
            assert!(!m.brain.has_pending(), "aucune attente créée");
        }
        assert!(m.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn outil_interdit_par_le_role_refuse() {
        let m = montage(FakeLlm::new(Ok(tool_understanding("web_search"))));
        m.avec_lea();
        let a = m
            .brain
            .answer_as(&ear(), "cherche des recettes", &m.lea(Trust::Unverified))
            .await;
        refus(&a);
        assert!(m.media.calls().is_empty());
        // Même chose par le raccourci du cerveau d'un outil hors du rôle.
        let a = m
            .brain
            .answer_fast_as(&ear(), VEILLE, &m.lea(Trust::Unverified))
            .await;
        assert!(
            a.is_none(),
            "risqué : jamais proposé à une voix non vérifiée"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn outil_choisi_par_l_ia_mais_interdit_refuse() {
        let m = montage(FakeLlm::new(Ok(tool_understanding("suspend"))));
        m.avec_lea();
        let a = m
            .brain
            .answer_as(
                &ear(),
                "fais quelque chose d'utile",
                &m.lea(Trust::Recognized),
            )
            .await;
        refus(&a);
        assert!(!m.brain.has_pending());
        assert!(m.ran().await.is_empty());
        // Outil inventé, inconnu : phrase habituelle, rien n'est lancé.
        let m2 = montage(FakeLlm::new(Ok(tool_understanding("inexistant"))));
        m2.avec_lea();
        let a = m2
            .brain
            .answer_as(&ear(), "fais un truc", &m2.lea(Trust::Unverified))
            .await;
        assert_eq!(a.reply.outcome, Outcome::Failed);
        assert_eq!(
            a.reply.speech,
            "Je crains de ne pas savoir faire cela, Monsieur."
        );
    }

    #[tokio::test(start_paused = true)]
    async fn l_ia_recoit_tous_les_outils_pour_un_invite() {
        let m = montage(bavard());
        m.avec_lea();
        m.brain
            .answer_as(&ear(), "question", &m.people.typed())
            .await;
        let admin = m.llm.last_tools();
        m.brain
            .answer_as(&ear(), "autre question", &m.lea(Trust::Unverified))
            .await;
        assert_eq!(m.llm.last_tools(), admin);
        assert!(!admin.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn agent_refuse_sans_droit_agent() {
        let m = montage(bavard());
        m.avec_lea();
        for trust in [Trust::Unverified, Trust::Accepted, Trust::Recognized] {
            let a = m.brain.answer_as(&ear(), PHRASE_AGENT, &m.lea(trust)).await;
            refus(&a);
        }
        assert!(m.agent.calls().is_empty());
        assert_eq!(m.llm.calls(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn agent_exige_voix_reconnue_meme_avec_le_droit() {
        let m = montage(bavard());
        m.role("dev", &["media_*"], false, true);
        m.people.add_person("Max", "dev").unwrap();
        for trust in [Trust::Unverified, Trust::Accepted] {
            let a = m
                .brain
                .answer_as(&ear(), PHRASE_AGENT, &m.people.speaker("Max", trust))
                .await;
            assert_eq!(a.reply.speech, NEEDS_KNOWN_VOICE);
        }
        assert!(m.agent.calls().is_empty());
        m.brain
            .answer_as(
                &ear(),
                PHRASE_AGENT,
                &m.people.speaker("Max", Trust::Recognized),
            )
            .await;
        assert_eq!(m.agent.calls().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn texte_tape_vaut_admin_et_peut_l_agent() {
        let m = montage(bavard());
        m.brain.handle_text(PHRASE_AGENT).await;
        assert_eq!(m.agent.calls().len(), 1);
        m.brain.handle_text("suivant").await;
        assert_eq!(m.media.calls().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn texte_tape_risque_toujours_a_voix_haute() {
        let m = montage(bavard());
        let r = m.brain.handle_text(VEILLE).await;
        assert_eq!(
            r.speech,
            "Pour cela, demandez-le-moi à voix haute, Monsieur."
        );
        assert!(!m.brain.has_pending());
        assert!(m.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn voix_non_verifiee_admin_jamais_risque() {
        let m = montage(bavard());
        let who = m.people.owner_speaker(Trust::Unverified);
        assert!(m.brain.answer_fast_as(&ear(), VEILLE, &who).await.is_none());
        let a = m.brain.answer_as(&ear(), VEILLE, &who).await;
        refus(&a);
        assert!(!m.brain.has_pending());
        assert!(m.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn confirmation_sans_droit_refusee() {
        let m = montage(bavard());
        m.role("veille", &["suspend"], false, false);
        m.people.add_person("Max", "veille").unwrap();
        // Charles pose la question ; Max (outil permis, pas de droit confirm) répond oui.
        let q = m
            .brain
            .answer_as(&ear(), VEILLE, &m.people.owner_speaker(Trust::Recognized))
            .await;
        assert!(q.awaits_reply);
        let max = m.people.speaker("Max", Trust::Recognized);
        let a = m
            .brain
            .answer_follow_up_as(&ear(), "oui", m.brain.now(), ReplyVoice::Verified, &max)
            .await;
        refus(&a);
        assert!(!m.brain.has_pending());
        assert!(m.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn role_retire_entre_question_et_oui_refuse() {
        let m = montage(bavard());
        m.role("veille", &["suspend"], true, false);
        m.people.add_person("Max", "veille").unwrap();
        let q = m
            .brain
            .answer_as(&ear(), VEILLE, &m.people.speaker("Max", Trust::Recognized))
            .await;
        assert!(q.awaits_reply);
        m.people
            .edit_role("veille", None, Some(false), None, &m.tools())
            .unwrap();
        let a = m
            .brain
            .answer_follow_up_as(
                &ear(),
                "oui",
                m.brain.now(),
                ReplyVoice::Verified,
                &m.people.speaker("Max", Trust::Recognized),
            )
            .await;
        refus(&a);
        assert!(m.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn confirmation_avec_droit_execute() {
        let m = montage(bavard());
        m.role("veille", &["suspend"], true, false);
        m.people.add_person("Max", "veille").unwrap();
        let max = m.people.speaker("Max", Trust::Recognized);
        m.brain.answer_as(&ear(), VEILLE, &max).await;
        m.brain
            .answer_follow_up_as(&ear(), "oui", m.brain.now(), ReplyVoice::Verified, &max)
            .await;
        assert_eq!(m.ran().await.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn oui_d_une_autre_oreille_n_execute_pas() {
        let m = montage(bavard());
        let q = m.brain.answer_text(&EarId::new("salon"), VEILLE).await;
        assert!(q.awaits_reply);
        let a = m
            .brain
            .answer_follow_up_as(
                &EarId::new("chambre"),
                "oui",
                m.brain.now(),
                ReplyVoice::Verified,
                &m.people.owner_speaker(Trust::Recognized),
            )
            .await;
        assert_eq!(a.reply.speech, "");
        assert!(!m.brain.has_pending());
        assert!(m.ran().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn cerveau_par_defaut_se_comporte_comme_avant() {
        let m = montage(bavard());
        let q = m.brain.answer_text(&ear(), VEILLE).await;
        assert!(q.awaits_reply);
        m.brain
            .answer_follow_up(&ear(), "oui", m.brain.now(), ReplyVoice::Verified)
            .await;
        assert_eq!(m.ran().await.len(), 1);
        // Voix non vérifiée : toujours pas de risqué, mais le reste passe.
        assert!(m.brain.answer_fast(&ear(), VEILLE).await.is_none());
        m.brain.answer_voice(&ear(), PHRASE_AGENT, true).await;
        assert_eq!(m.agent.calls().len(), 1);
        m.brain.answer_voice(&ear(), PHRASE_AGENT, false).await;
        assert_eq!(m.agent.calls().len(), 1);
    }
}
