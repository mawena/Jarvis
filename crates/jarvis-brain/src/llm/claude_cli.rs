//! Claude par la CLI `claude -p` (Claude Code), avec le compte actif de CCS.
//!
//! Pourquoi la CLI plutôt que l'API : les comptes Claude de Charles sont des abonnements
//! gérés par CCS (`ccs meilleur`, bascule automatique), sans clé d'API. `claude -p`
//! utilise le compte actif ; quand il bute sur une limite, `ccs meilleur` bascule sur le
//! compte le plus disponible et l'appel est refait une fois. S'il n'y en a aucun, l'erreur
//! [`LlmError::Quota`] laisse le routeur ([`super::appoint`]) se replier sur l'IA locale.
//!
//! Les outils de Jarvis sont décrits dans la consigne ; Claude répond en JSON structuré
//! (`--json-schema`) : `{"outil", "arguments"}` ou `{"reponse"}`. Aucun outil de Claude
//! Code n'est ouvert (`--tools ""`), aucun serveur MCP, aucune session gardée.

use super::{LlmBackend, LlmError, Understanding, system_prompt};
use crate::config::ClaudeCliConfig;
use crate::intent::Intent;
use jarvis_skills::ToolSpec;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Sortie d'un programme lancé.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sortie {
    /// Code de sortie (`None` : tué par un signal).
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Lance un programme : abstrait pour que les tests n'appellent jamais le vrai Claude.
#[async_trait::async_trait]
pub trait Lanceur: Send + Sync {
    /// `Err` : programme introuvable, impossible à lancer, ou délai dépassé.
    async fn lancer(
        &self,
        programme: &str,
        args: &[String],
        entree: &str,
        delai: Duration,
    ) -> Result<Sortie, String>;
}

/// Lanceur réel (`tokio::process`), dans le dossier temporaire : aucun `CLAUDE.md` de
/// projet n'est lu. Le processus est tué si le délai est dépassé.
pub struct LanceurSysteme;

#[async_trait::async_trait]
impl Lanceur for LanceurSysteme {
    async fn lancer(
        &self,
        programme: &str,
        args: &[String],
        entree: &str,
        delai: Duration,
    ) -> Result<Sortie, String> {
        use tokio::io::AsyncWriteExt;
        let mut child = tokio::process::Command::new(programme)
            .args(args)
            .current_dir(std::env::temp_dir())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("{programme} : {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(entree.as_bytes())
                .await
                .map_err(|e| format!("{programme} : écriture : {e}"))?;
        }
        match tokio::time::timeout(delai, child.wait_with_output()).await {
            Ok(Ok(out)) => Ok(Sortie {
                code: out.status.code(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
            Ok(Err(e)) => Err(format!("{programme} : {e}")),
            Err(_) => Err(format!(
                "{programme} : délai de {} s dépassé",
                delai.as_secs()
            )),
        }
    }
}

/// Schéma de la réponse structurée demandée à Claude.
fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "outil": {"type": "string"},
            "arguments": {"type": "object"},
            "reponse": {"type": "string"}
        },
        "additionalProperties": false
    })
}

/// Consigne : le ton de Jarvis, puis les outils comme données.
fn consigne(address: &str, tools: &[ToolSpec]) -> String {
    let outils: Vec<Value> = tools
        .iter()
        .map(|t| json!({"nom": t.name, "description": t.description, "arguments": t.parameters}))
        .collect();
    format!(
        "{} Tu n'exécutes rien toi-même : tu choisis seulement. Si la demande correspond à \
         un des outils de Jarvis ci-dessous, réponds avec \"outil\" (son nom exact) et \
         \"arguments\" ; Jarvis l'exécutera. Sinon réponds avec \"reponse\", la phrase à \
         dire. Outils de Jarvis : {}",
        system_prompt(address),
        Value::Array(outils)
    )
}

/// Un message d'erreur de Claude Code annonce-t-il une limite d'usage ?
fn est_limite(texte: &str) -> bool {
    let t = texte.to_lowercase();
    [
        "usage limit",
        "limit reached",
        "hit your limit",
        "rate limit",
        "rate_limit",
        "quota",
        "overloaded",
        "credit balance",
        "out of extra usage",
    ]
    .iter()
    .any(|m| t.contains(m))
}

pub struct ClaudeCliBackend {
    cfg: ClaudeCliConfig,
    address: String,
    lanceur: Arc<dyn Lanceur>,
}

impl ClaudeCliBackend {
    pub fn new(cfg: &ClaudeCliConfig, address: &str, lanceur: Arc<dyn Lanceur>) -> Self {
        Self {
            cfg: cfg.clone(),
            address: address.to_string(),
            lanceur,
        }
    }

    /// Avec le vrai lanceur de processus.
    pub fn systeme(cfg: &ClaudeCliConfig, address: &str) -> Self {
        Self::new(cfg, address, Arc::new(LanceurSysteme))
    }

    fn delai(&self) -> Duration {
        Duration::from_secs(self.cfg.delai_s.max(1))
    }

    fn args(&self, system: &str, avec_schema: bool) -> Vec<String> {
        let mut a: Vec<String> = [
            "-p",
            "--model",
            &self.cfg.modele,
            "--tools",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--output-format",
            "json",
            "--system-prompt",
            system,
        ]
        .map(String::from)
        .to_vec();
        if avec_schema {
            a.push("--json-schema".into());
            a.push(schema().to_string());
        }
        a
    }

    /// Un appel de `claude -p`, lu en JSON (`--output-format json`).
    async fn appel(&self, args: &[String], entree: &str) -> Result<Value, LlmError> {
        let sortie = self
            .lanceur
            .lancer(&self.cfg.commande, args, entree, self.delai())
            .await
            .map_err(LlmError::Unreachable)?;
        let v: Option<Value> = serde_json::from_str(sortie.stdout.trim()).ok();
        let en_erreur = sortie.code != Some(0)
            || v.as_ref()
                .is_some_and(|v| v.get("is_error").and_then(Value::as_bool) == Some(true));
        if en_erreur {
            let message = v
                .as_ref()
                .and_then(|v| v.get("result").and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or_else(|| format!("{} {}", sortie.stdout, sortie.stderr));
            let message = message.trim().chars().take(300).collect::<String>();
            return Err(if est_limite(&message) {
                LlmError::Quota(message)
            } else {
                LlmError::Unreachable(message)
            });
        }
        v.ok_or_else(|| LlmError::BadResponse("sortie de claude -p illisible".into()))
    }

    /// Appel, et sur une limite : `ccs meilleur` puis un second essai.
    async fn appel_avec_bascule(&self, args: &[String], entree: &str) -> Result<Value, LlmError> {
        match self.appel(args, entree).await {
            Err(LlmError::Quota(m)) if !self.cfg.ccs.trim().is_empty() => {
                info!(raison = %m, "limite de Claude : bascule par ccs meilleur");
                let ccs = self
                    .lanceur
                    .lancer(
                        self.cfg.ccs.trim(),
                        &["meilleur".to_string()],
                        "",
                        Duration::from_secs(30),
                    )
                    .await;
                match ccs {
                    Ok(s) if s.code == Some(0) => self.appel(args, entree).await,
                    Ok(s) => Err(LlmError::Quota(format!(
                        "aucun compte Claude disponible : {}",
                        s.stdout.trim().lines().last().unwrap_or("")
                    ))),
                    Err(e) => {
                        warn!(erreur = %e, "ccs meilleur impossible");
                        Err(LlmError::Quota(m))
                    }
                }
            }
            r => r,
        }
    }
}

/// Réponse structurée → compréhension.
fn lire_comprehension(v: &Value) -> Result<Understanding, LlmError> {
    let structure = match v.get("structured_output") {
        Some(s) if s.is_object() => s.clone(),
        // Ancienne CLI ou schéma ignoré : le texte peut contenir le JSON lui-même.
        _ => {
            let texte = v.get("result").and_then(Value::as_str).unwrap_or("").trim();
            match serde_json::from_str::<Value>(texte) {
                Ok(s) if s.is_object() => s,
                _ if !texte.is_empty() => return Ok(Understanding::Answer(texte.to_string())),
                _ => return Err(LlmError::BadResponse("réponse vide de Claude".into())),
            }
        }
    };
    if let Some(outil) = structure
        .get("outil")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|o| !o.is_empty())
    {
        let args = structure
            .get("arguments")
            .filter(|a| a.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        return Ok(Understanding::Tool(Intent::new(outil, args)));
    }
    match structure
        .get("reponse")
        .and_then(Value::as_str)
        .map(str::trim)
    {
        Some(r) if !r.is_empty() => Ok(Understanding::Answer(r.to_string())),
        _ => Err(LlmError::BadResponse(
            "ni outil ni réponse de Claude".into(),
        )),
    }
}

#[async_trait::async_trait]
impl LlmBackend for ClaudeCliBackend {
    async fn understand(&self, text: &str, tools: &[ToolSpec]) -> Result<Understanding, LlmError> {
        let args = self.args(&consigne(&self.address, tools), true);
        let v = self.appel_avec_bascule(&args, text).await?;
        let u = lire_comprehension(&v)?;
        debug!(?u, "compris par Claude (CLI)");
        Ok(u)
    }

    async fn complete(&self, system: &str, user: &str, _max: u32) -> Result<String, LlmError> {
        let v = self
            .appel_avec_bascule(&self.args(system, false), user)
            .await?;
        match v.get("result").and_then(Value::as_str).map(str::trim) {
            Some(t) if !t.is_empty() => Ok(t.to_string()),
            _ => Err(LlmError::BadResponse("réponse vide de Claude".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Lanceur programmé : une sortie par appel, dans l'ordre ; garde les appels.
    struct FauxLanceur {
        sorties: Mutex<Vec<Result<Sortie, String>>>,
        appels: Mutex<Vec<(String, Vec<String>, String)>>,
    }

    impl FauxLanceur {
        fn new(sorties: Vec<Result<Sortie, String>>) -> Arc<Self> {
            Arc::new(Self {
                sorties: Mutex::new(sorties),
                appels: Mutex::new(Vec::new()),
            })
        }
        fn programmes(&self) -> Vec<String> {
            self.appels
                .lock()
                .unwrap()
                .iter()
                .map(|a| a.0.clone())
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl Lanceur for FauxLanceur {
        async fn lancer(
            &self,
            programme: &str,
            args: &[String],
            entree: &str,
            _: Duration,
        ) -> Result<Sortie, String> {
            self.appels
                .lock()
                .unwrap()
                .push((programme.into(), args.to_vec(), entree.into()));
            let mut s = self.sorties.lock().unwrap();
            if s.is_empty() {
                return Err("aucune sortie programmée".into());
            }
            s.remove(0)
        }
    }

    fn ok(stdout: Value) -> Result<Sortie, String> {
        Ok(Sortie {
            code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
        })
    }

    fn limite() -> Result<Sortie, String> {
        Ok(Sortie {
            code: Some(1),
            stdout: json!({"type": "result", "is_error": true,
                           "result": "Claude AI usage limit reached|1700000000"})
            .to_string(),
            stderr: String::new(),
        })
    }

    fn code(c: i32) -> Result<Sortie, String> {
        Ok(Sortie {
            code: Some(c),
            stdout: "Aucun compte utilisable pour l'instant.".into(),
            stderr: String::new(),
        })
    }

    fn backend(l: Arc<FauxLanceur>) -> ClaudeCliBackend {
        ClaudeCliBackend::new(&ClaudeCliConfig::default(), "Monsieur", l)
    }

    fn outils() -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "media_pause".into(),
            description: "met en pause".into(),
            parameters: json!({"type": "object", "properties": {}}),
            risk: jarvis_skills::Risk::Direct,
        }]
    }

    #[tokio::test]
    async fn outil_structure() {
        let l = FauxLanceur::new(vec![ok(json!({"is_error": false,
            "structured_output": {"outil": "media_pause", "arguments": {}}}))]);
        let u = backend(l.clone())
            .understand("mets en pause", &outils())
            .await
            .unwrap();
        assert_eq!(
            u,
            Understanding::Tool(Intent::new("media_pause", json!({})))
        );
        let appels = l.appels.lock().unwrap();
        let (prog, args, entree) = &appels[0];
        assert_eq!(prog, "claude");
        assert_eq!(entree, "mets en pause");
        // Aucun outil de Claude Code ouvert, consigne avec les outils de Jarvis.
        let i = args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(args[i + 1], "");
        assert!(args.iter().any(|a| a.contains("media_pause")));
        assert!(args.iter().any(|a| a == "--json-schema"));
    }

    #[tokio::test]
    async fn reponse_libre() {
        let l = FauxLanceur::new(vec![ok(json!({"is_error": false,
            "structured_output": {"reponse": "Lima, Monsieur."}}))]);
        let u = backend(l)
            .understand("capitale du Pérou", &[])
            .await
            .unwrap();
        assert_eq!(u, Understanding::Answer("Lima, Monsieur.".into()));
    }

    #[tokio::test]
    async fn json_dans_le_texte_accepte() {
        let l = FauxLanceur::new(vec![ok(
            json!({"is_error": false, "result": "{\"reponse\":\"Oui.\"}"}),
        )]);
        let u = backend(l).understand("ça va ?", &[]).await.unwrap();
        assert_eq!(u, Understanding::Answer("Oui.".into()));
    }

    #[tokio::test]
    async fn limite_puis_bascule_ccs_puis_succes() {
        let l = FauxLanceur::new(vec![
            limite(),
            code(0),
            ok(json!({"is_error": false, "structured_output": {"reponse": "Bien."}})),
        ]);
        let u = backend(l.clone()).understand("salut", &[]).await.unwrap();
        assert_eq!(u, Understanding::Answer("Bien.".into()));
        assert_eq!(l.programmes(), ["claude", "ccs", "claude"]);
        assert_eq!(l.appels.lock().unwrap()[1].1, vec!["meilleur".to_string()]);
    }

    #[tokio::test]
    async fn limite_sans_aucun_compte_donne_quota() {
        let l = FauxLanceur::new(vec![limite(), code(1)]);
        let e = backend(l.clone())
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(e, LlmError::Quota(_)), "{e:?}");
        assert_eq!(l.programmes(), ["claude", "ccs"]);
    }

    #[tokio::test]
    async fn sans_ccs_pas_de_bascule() {
        let l = FauxLanceur::new(vec![limite()]);
        let cfg = ClaudeCliConfig {
            ccs: String::new(),
            ..ClaudeCliConfig::default()
        };
        let e = ClaudeCliBackend::new(&cfg, "Monsieur", l.clone())
            .understand("salut", &[])
            .await
            .unwrap_err();
        assert!(matches!(e, LlmError::Quota(_)), "{e:?}");
        assert_eq!(l.programmes(), ["claude"]);
    }

    #[tokio::test]
    async fn claude_introuvable_ou_delai_injoignable() {
        let l = FauxLanceur::new(vec![Err("claude : délai de 20 s dépassé".into())]);
        let e = backend(l).understand("salut", &[]).await.unwrap_err();
        assert!(matches!(e, LlmError::Unreachable(_)), "{e:?}");
    }

    #[tokio::test]
    async fn sortie_illisible_ou_vide() {
        let l = FauxLanceur::new(vec![Ok(Sortie {
            code: Some(0),
            stdout: "pas du json".into(),
            stderr: String::new(),
        })]);
        let e = backend(l).understand("salut", &[]).await.unwrap_err();
        assert!(matches!(e, LlmError::BadResponse(_)), "{e:?}");
        let l = FauxLanceur::new(vec![ok(json!({"is_error": false,
            "structured_output": {}}))]);
        let e = backend(l).understand("salut", &[]).await.unwrap_err();
        assert!(matches!(e, LlmError::BadResponse(_)), "{e:?}");
    }

    #[test]
    fn detection_des_limites() {
        assert!(est_limite("Claude AI usage limit reached|123"));
        assert!(est_limite("You've hit your limit · resets 5pm"));
        assert!(!est_limite("Connection error"));
    }
}
