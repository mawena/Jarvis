//! `jarvis pair` (fenêtre d'appairage) et `jarvis ears` (tableau des oreilles).

use jarvis_protocol::EarSummary;
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use std::io::{IsTerminal, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Chiffres en gros (5 lignes, 3 colonnes).
const DIGITS: [[&str; 5]; 10] = [
    ["███", "█ █", "█ █", "█ █", "███"],
    ["  █", "  █", "  █", "  █", "  █"],
    ["███", "  █", "███", "█  ", "███"],
    ["███", "  █", "███", "  █", "███"],
    ["█ █", "█ █", "███", "  █", "  █"],
    ["███", "█  ", "███", "  █", "███"],
    ["███", "█  ", "███", "█ █", "███"],
    ["███", "  █", "  █", "  █", "  █"],
    ["███", "█ █", "███", "█ █", "███"],
    ["███", "█ █", "███", "  █", "███"],
];

/// Le code en gros : chiffres espacés, par groupes de trois.
pub fn big_code(code: &str) -> String {
    let mut rows = vec![String::new(); 5];
    for (i, c) in code.chars().enumerate() {
        let glyph = c.to_digit(10).map(|d| DIGITS[d as usize]);
        for (row, line) in rows.iter_mut().enumerate() {
            if i > 0 {
                line.push_str(if i % 3 == 0 { "    " } else { "  " });
            }
            line.push_str(glyph.map_or("   ", |g| g[row]));
        }
    }
    rows.iter()
        .map(|r| format!("  {}", r.trim_end()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// QR code de l'URI en demi-blocs Unicode (modules sombres = `█`, zone blanche
/// autour). Au terminal, encadré de noir sur blanc pour être lisible sur fond sombre.
pub fn qr_code(uri: &str, colors: bool) -> Option<String> {
    let image = QrCode::new(uri.as_bytes())
        .ok()?
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Dark)
        .light_color(Dense1x2::Light)
        .quiet_zone(true)
        .build();
    if !colors {
        return Some(image);
    }
    Some(
        image
            .lines()
            .map(|l| format!("\x1b[30;47m{l}\x1b[0m"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// « 2 min 00 s ».
pub fn remaining(secs: u64) -> String {
    format!("{} min {:02} s", secs / 60, secs % 60)
}

/// Écran de la fenêtre d'appairage.
pub fn screen(code: &str, uri: &str, fingerprint_short: &str, colors: bool) -> String {
    let qr = qr_code(uri, colors).unwrap_or_else(|| "(QR code impossible, URI trop longue)".into());
    format!(
        "Appairage d'une oreille\n\n{}\n\nCode : {code}\n\n{qr}\n\n\
         Sur le téléphone : scanner ce QR code, ou copier cette adresse :\n{uri}\n\n\
         Empreinte du PC : {fingerprint_short}\n\
         (à comparer avec celle affichée par l'oreille)\n\n\
         Ctrl-C pour fermer la fenêtre.",
        big_code(code)
    )
}

/// Compte à rebours sur une ligne (terminal) ; sinon, la durée une seule fois.
/// S'arrête quand l'émetteur rendu est lâché.
pub fn countdown(expires_in_secs: u64) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (stop, stopped) = mpsc::channel::<()>();
    let tty = std::io::stdout().is_terminal();
    let handle = std::thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(expires_in_secs);
        let mut out = std::io::stdout();
        if !tty {
            let _ = writeln!(
                out,
                "La fenêtre se ferme dans {}.",
                remaining(expires_in_secs)
            );
            let _ = stopped.recv();
            return;
        }
        loop {
            let left = end.saturating_duration_since(Instant::now()).as_secs();
            let _ = write!(out, "\rLa fenêtre se ferme dans {}. ", remaining(left));
            let _ = out.flush();
            if !matches!(
                stopped.recv_timeout(Duration::from_secs(1)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                let _ = writeln!(out);
                return;
            }
        }
    });
    (stop, handle)
}

/// Tableau nom / plateforme / connectée / locale.
pub fn ears_table(ears: &[EarSummary]) -> String {
    if ears.is_empty() {
        return "Aucune oreille appairée.".into();
    }
    let yes_no = |b: bool| if b { "oui" } else { "non" }.to_string();
    let mut rows = vec![[
        "Nom".to_string(),
        "Plateforme".to_string(),
        "Connectée".to_string(),
        "Locale".to_string(),
    ]];
    rows.extend(ears.iter().map(|e| {
        [
            e.name.clone(),
            e.platform.clone(),
            yes_no(e.connected),
            yes_no(e.local),
        ]
    }));
    let widths: Vec<usize> = (0..4)
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    rows.iter()
        .map(|r| {
            let mut line = String::new();
            for (c, cell) in r.iter().enumerate() {
                line.push_str(cell);
                let pad = widths[c] - cell.chars().count() + 2;
                line.push_str(&" ".repeat(pad));
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const URI: &str = "jarvis://pair?v=1&id=abc&h=192.168.1.20&p=7341&fp=xyz&c=482913";

    #[test]
    fn qr_code_contient_l_uri() {
        let qr = qr_code(URI, false).unwrap();
        // Une ligne pour deux rangées de modules, zone blanche de 4 de chaque côté.
        let width = QrCode::new(URI.as_bytes()).unwrap().width() + 8;
        assert_eq!(qr.lines().count(), width.div_ceil(2));
        assert!(qr.lines().all(|l| l.chars().count() == width));
        assert_ne!(qr, qr_code("jarvis://pair?v=1&c=000000", false).unwrap());
        let screen = screen("482913", URI, "A1B2-C3D4-E5F6-0718", false);
        assert!(screen.contains(&qr));
        assert!(screen.contains(URI));
    }

    #[test]
    fn qr_code_en_couleurs_au_terminal() {
        let qr = qr_code(URI, true).unwrap();
        assert!(
            qr.lines()
                .all(|l| l.starts_with("\x1b[30;47m") && l.ends_with("\x1b[0m"))
        );
    }

    #[test]
    fn code_en_gros() {
        let big = big_code("10");
        assert_eq!(big.lines().count(), 5);
        assert_eq!(big.lines().next().unwrap(), "    █  ███");
        // Groupes de trois chiffres séparés.
        assert!(
            big_code("123456")
                .lines()
                .next()
                .unwrap()
                .contains("███    █ █")
        );
    }

    #[test]
    fn duree_restante() {
        assert_eq!(remaining(120), "2 min 00 s");
        assert_eq!(remaining(65), "1 min 05 s");
    }
}
