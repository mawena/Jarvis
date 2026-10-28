# IA locale et jeu de phrases de référence

## Installer Ollama

```bash
curl -fsSL https://ollama.com/install.sh | sh
ollama pull qwen2.5:3b
ollama pull llama3.2:3b
```

## Mesurer un modèle

Le jeu de phrases (`crates/jarvis-brain/tests/fixtures/phrases.toml`) est rejoué contre le modèle ;
le test échoue sous 80 % de réussite et affiche le temps médian.

```bash
# Local (Ollama)
JARVIS_TEST_MODEL=qwen2.5:3b cargo test -p jarvis-brain --test phrases -- --ignored ollama_jeu_de_phrases --nocapture
JARVIS_TEST_MODEL=llama3.2:3b cargo test -p jarvis-brain --test phrases -- --ignored ollama_jeu_de_phrases --nocapture

# En ligne (Claude)
ANTHROPIC_API_KEY=... cargo test -p jarvis-brain --test phrases -- --ignored claude_jeu_de_phrases --nocapture
```

## Résultats

| Modèle | Réussite | Temps médian |
|--------|----------|--------------|
|        |          |              |
