//! Jeu de phrases de référence : raccourcis (toujours) et IA réelle (`--ignored`).

use async_trait::async_trait;
use jarvis_brain::config::{LocalLlmConfig, OnlineLlmConfig};
use jarvis_brain::fastpath::match_fast;
use jarvis_brain::intent::Intent;
use jarvis_brain::llm::{ClaudeBackend, LlmBackend, OllamaBackend, Understanding};
use jarvis_skills::apps::{AppIndex, AppsSkill};
use jarvis_skills::media::{MediaPlayers, MediaSkill, PlayerCommand, PlayerInfo};
use jarvis_skills::{FakeRunner, Skill, ToolSpec};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
struct Fixture {
    phrase: Vec<Phrase>,
}

#[derive(Deserialize)]
struct Phrase {
    text: String,
    tool: String,
    args: serde_json::Value,
    fast: bool,
}

fn phrases() -> Vec<Phrase> {
    let raw = include_str!("fixtures/phrases.toml");
    toml::from_str::<Fixture>(raw).expect("phrases.toml").phrase
}

struct NoPlayers;

#[async_trait]
impl MediaPlayers for NoPlayers {
    async fn players(&self) -> anyhow::Result<Vec<PlayerInfo>> {
        Ok(vec![])
    }
    async fn command(&self, _bus_name: &str, _cmd: PlayerCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Les vrais outils exposés au modèle, avec des dépendances factices.
fn real_tools() -> Vec<ToolSpec> {
    let runner = Arc::new(FakeRunner::new());
    let media = MediaSkill::new(Arc::new(NoPlayers), runner.clone());
    let apps = AppsSkill::new(AppIndex::load(&[]), HashMap::new(), runner);
    [media.tools(), apps.tools()].concat()
}

#[test]
fn le_jeu_de_phrases_est_assez_fourni() {
    let all = phrases();
    assert!(all.len() >= 30, "{} phrases", all.len());
    assert!(all.iter().filter(|p| !p.fast).count() >= 10);
}

#[test]
fn raccourcis_couvrent_leurs_phrases() {
    for p in phrases().iter().filter(|p| p.fast) {
        assert_eq!(
            match_fast(&p.text),
            Some(Intent::new(p.tool.clone(), p.args.clone())),
            "phrase: {}",
            p.text
        );
    }
}

#[test]
fn phrases_ia_ne_passent_pas_par_les_raccourcis() {
    for p in phrases().iter().filter(|p| !p.fast) {
        assert_eq!(match_fast(&p.text), None, "phrase: {}", p.text);
    }
}

async fn run_set(label: &str, backend: &dyn LlmBackend) {
    let tools = real_tools();
    // Les outils système ne sont pas proposés à l'IA : leurs phrases passent par les raccourcis.
    let all: Vec<Phrase> = phrases()
        .into_iter()
        .filter(|p| tools.iter().any(|t| t.name == p.tool))
        .collect();
    let mut ok = 0usize;
    let mut times: Vec<Duration> = Vec::new();
    for p in &all {
        let start = Instant::now();
        let result = backend.understand(&p.text, &tools).await;
        times.push(start.elapsed());
        match result {
            Ok(Understanding::Tool(i)) if i.tool == p.tool => ok += 1,
            other => println!("ECHEC {:?} : attendu {}, obtenu {other:?}", p.text, p.tool),
        }
    }
    times.sort();
    let median = times[times.len() / 2];
    let rate = ok as f64 / all.len() as f64;
    println!(
        "{label} : {ok}/{} réussites ({:.0} %), temps médian {} ms",
        all.len(),
        rate * 100.0,
        median.as_millis()
    );
    assert!(rate >= 0.80, "taux de réussite {rate:.2} < 0.80");
}

#[tokio::test]
#[ignore = "nécessite Ollama et un modèle installé"]
async fn ollama_jeu_de_phrases() {
    let mut cfg = LocalLlmConfig::default();
    if let Ok(model) = std::env::var("JARVIS_TEST_MODEL") {
        cfg.model = model;
    }
    let label = format!("ollama {}", cfg.model);
    run_set(&label, &OllamaBackend::new(&cfg, "Charles")).await;
}

#[tokio::test]
#[ignore = "nécessite ANTHROPIC_API_KEY et le réseau"]
async fn claude_jeu_de_phrases() {
    let cfg = OnlineLlmConfig::default();
    let label = format!("claude {}", cfg.model);
    run_set(&label, &ClaudeBackend::from_env(&cfg, "Charles")).await;
}
