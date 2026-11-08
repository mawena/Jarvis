//! URI d'appairage, portée par le QR code de `jarvis pair` :
//! `jarvis://pair?v=1&id=…&h=192.168.1.20,10.0.0.5&p=7341&fp=…&c=482913`.
//! Que de l'ASCII sûr (pas de nom), donc rien à échapper.

use crate::fingerprint::Fingerprint;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

const PREFIX: &str = "jarvis://pair?";
/// Version de l'URI (indépendante de celle du protocole).
const URI_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingUri {
    pub brain_id: String,
    /// Adresses du cerveau ; vide : découverte mDNS seulement.
    pub hosts: Vec<IpAddr>,
    pub port: u16,
    pub fingerprint: Fingerprint,
    /// Code d'appairage (6 chiffres).
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairingUriError {
    #[error("ce n'est pas une adresse d'appairage Jarvis")]
    NotJarvis,
    #[error("version d'appairage inconnue")]
    Version,
    #[error("paramètre manquant : {0}")]
    Missing(&'static str),
    #[error("paramètre invalide : {0}")]
    Invalid(&'static str),
}

/// Identifiant du cerveau : ASCII sûr seulement (hexadécimal en pratique).
fn is_safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl PairingUri {
    pub fn parse(s: &str) -> Result<Self, PairingUriError> {
        use PairingUriError::{Invalid, Missing};
        let query = s.strip_prefix(PREFIX).ok_or(PairingUriError::NotJarvis)?;
        let mut params: Vec<(&str, &str)> = Vec::new();
        for kv in query.split('&') {
            let (k, v) = kv.split_once('=').ok_or(Invalid("format"))?;
            if params.iter().any(|(seen, _)| *seen == k) {
                return Err(Invalid("doublon"));
            }
            params.push((k, v));
        }
        let get = |key: &'static str| {
            params
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| *v)
                .ok_or(Missing(key))
        };
        if get("v")? != URI_VERSION {
            return Err(PairingUriError::Version);
        }
        let brain_id = get("id")?;
        if !is_safe_id(brain_id) {
            return Err(Invalid("id"));
        }
        let hosts = match get("h")? {
            "" => Vec::new(),
            h => h
                .split(',')
                .map(|ip| ip.parse::<IpAddr>().map_err(|_| Invalid("h")))
                .collect::<Result<_, _>>()?,
        };
        let port = get("p")?
            .parse::<u16>()
            .ok()
            .filter(|&p| p != 0)
            .ok_or(Invalid("p"))?;
        let fingerprint = Fingerprint::parse_b64(get("fp")?).map_err(|_| Invalid("fp"))?;
        let code = get("c")?;
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Invalid("c"));
        }
        Ok(PairingUri {
            brain_id: brain_id.into(),
            hosts,
            port,
            fingerprint,
            code: code.into(),
        })
    }
}

impl fmt::Display for PairingUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hosts: Vec<String> = self.hosts.iter().map(IpAddr::to_string).collect();
        write!(
            f,
            "{PREFIX}v={URI_VERSION}&id={}&h={}&p={}&fp={}&c={}",
            self.brain_id,
            hosts.join(","),
            self.port,
            self.fingerprint.to_b64(),
            self.code
        )
    }
}

impl FromStr for PairingUri {
    type Err = PairingUriError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PairingUri::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> PairingUri {
        PairingUri {
            brain_id: "0123456789abcdef0123456789abcdef".into(),
            hosts: vec!["192.168.1.20".parse().unwrap(), "fe80::1".parse().unwrap()],
            port: 7341,
            fingerprint: Fingerprint::of_der(b"certificat"),
            code: "482913".into(),
        }
    }

    #[test]
    fn uri_appairage_aller_retour() {
        let u = uri();
        let s = u.to_string();
        assert_eq!(
            s,
            format!(
                "jarvis://pair?v=1&id=0123456789abcdef0123456789abcdef&h=192.168.1.20,fe80::1&p=7341&fp={}&c=482913",
                u.fingerprint.to_b64()
            )
        );
        assert!(s.is_ascii() && !s.contains(' '), "{s}");
        assert_eq!(PairingUri::parse(&s), Ok(u.clone()));
        assert_eq!(s.parse::<PairingUri>(), Ok(u.clone()));
        // Sans adresse : découverte par mDNS.
        let sans = PairingUri { hosts: vec![], ..u };
        assert_eq!(PairingUri::parse(&sans.to_string()), Ok(sans.clone()));
        // Paramètre ajouté par une version plus récente, ordre changé : lisible.
        let s = format!(
            "jarvis://pair?c=482913&x=1&p=7341&h=&id={}&fp={}&v=1",
            sans.brain_id,
            sans.fingerprint.to_b64()
        );
        assert_eq!(PairingUri::parse(&s), Ok(sans));
    }

    #[test]
    fn uri_appairage_incomplete_refusee() {
        let s = uri().to_string();
        for key in ["v", "id", "h", "p", "fp", "c"] {
            let partial: Vec<&str> = s[PREFIX.len()..]
                .split('&')
                .filter(|kv| !kv.starts_with(&format!("{key}=")))
                .collect();
            let partial = format!("{PREFIX}{}", partial.join("&"));
            assert!(PairingUri::parse(&partial).is_err(), "{partial}");
        }
        let fp = uri().fingerprint.to_b64();
        let bad = [
            "https://exemple.fr/pair?v=1".to_string(),
            String::new(),
            s.replace("v=1", "v=2"),
            s.replace("p=7341", "p=99999"),
            s.replace("p=7341", "p=0"),
            s.replace("c=482913", "c=48291"),
            s.replace("c=482913", "c=48291a"),
            s.replace(&fp, "abc"),
            s.replace("192.168.1.20", "pc.local"),
            s.replace("id=0123456789abcdef0123456789abcdef", "id="),
            s.replace("id=0123456789abcdef0123456789abcdef", "id=a%20b"),
            format!("{s}&c=111111"),
        ];
        for b in bad {
            assert!(PairingUri::parse(&b).is_err(), "{b}");
        }
    }

    /// Le texte d'une erreur d'analyse ne reprend jamais le code à usage unique ni le reste de l'adresse
    /// (il finit dans les journaux de l'oreille).
    #[test]
    fn erreurs_sans_le_code_ni_l_empreinte() {
        let bons = uri().to_string();
        let cassees = [
            bons.replace("p=7341", "p=0"),
            bons.replace("c=482913", "c=48291X"),
            bons.replace("h=192.168.1.20,fe80::1", "h=zzz"),
            bons.replace("id=0123456789abcdef0123456789abcdef", "id=a/b"),
            format!("{bons}&c=482913"),
            bons.replace("fp=", "fp=!!"),
        ];
        for s in cassees {
            let e = PairingUri::parse(&s).unwrap_err();
            for txt in [e.to_string(), format!("{e:?}")] {
                assert!(!txt.contains("482913") && !txt.contains("48291X"), "{txt}");
                assert!(!txt.contains(&uri().fingerprint.to_b64()), "{txt}");
            }
        }
    }
}
