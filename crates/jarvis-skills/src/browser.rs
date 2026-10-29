//! Navigateur du PC : ouverture d'une adresse web par `xdg-open`.

use crate::CommandRunner;
use reqwest::Url;

/// Adresse sûre à ouvrir : `http` ou `https`, avec un hôte. Sinon `None`.
pub fn checked_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw.trim()).ok()?;
    let web = matches!(url.scheme(), "http" | "https");
    let host = url.host_str().is_some_and(|h| !h.is_empty());
    (web && host).then_some(url)
}

/// Ouvre l'adresse dans le navigateur par défaut, sans shell, en un seul argument.
pub fn open_url(runner: &dyn CommandRunner, url: &Url) -> std::io::Result<()> {
    runner.spawn_detached("xdg-open", &[url.as_str().to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeRunner;

    #[test]
    fn ouvre_url_http_via_xdg_open() {
        let runner = FakeRunner::new();
        let url = checked_url(" https://exemple.fr/a?b=c d ").unwrap();
        open_url(&runner, &url).unwrap();
        assert_eq!(
            runner.calls(),
            vec![(
                "xdg-open".to_string(),
                vec!["https://exemple.fr/a?b=c%20d".to_string()]
            )]
        );
        assert!(checked_url("http://127.0.0.1:8888/search?q=x").is_some());
    }

    #[test]
    fn refuse_url_non_http() {
        for raw in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "-x",
            "",
            "   ",
            "http://",
            "ftp://exemple.fr/",
            "data:text/html,<b>x</b>",
            "exemple.fr",
        ] {
            assert_eq!(checked_url(raw), None, "{raw}");
        }
    }
}
