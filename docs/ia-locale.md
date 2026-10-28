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

Mesuré le 2026-10-28 sur le portable de Charles (Intel Core Ultra 7 155U, sans GPU dédié, 14 Go de RAM), Ollama 0.40.12, 31 phrases.

| Modèle | Réussite | Temps médian |
|--------|----------|--------------|
| `llama3.2:3b` | 26/31 (84 %) | 2,4 s |
| `qwen2.5:3b` | 20/31 (65 %) | 3,0 s |

**Modèle retenu : `llama3.2:3b`** (défaut de `LocalLlmConfig`).

Constats :
- `qwen2.5:3b` répond souvent par une phrase au lieu d'appeler l'outil (« Voulez-vous que je la fasse maintenant ? »).
- `llama3.2:3b` se trompe parfois d'outil (« reprends » → pause, « qu'est-ce qui passe » → suivant) ; ces phrases courtes passent de toute façon par les raccourcis rapides, sans IA.
- **Premier chargement trop lent** : la toute première phrase après le démarrage d'Ollama (ou après un changement de modèle) dépasse le délai de 20 s et échoue. À corriger : préchauffer le modèle au démarrage du cerveau et garder le modèle chargé (`keep_alive`).
