//! Empreinte du certificat du cerveau : SHA-256 du certificat DER, épinglée par
//! l'oreille à l'appairage.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("empreinte invalide")]
pub struct FingerprintError;

impl Fingerprint {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Fingerprint(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// SHA-256 d'un certificat DER.
    pub fn of_der(der: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        let mut out = [0u8; 32];
        out.copy_from_slice(&Sha256::digest(der));
        Fingerprint(out)
    }

    /// Base64url sans remplissage (43 caractères).
    pub fn to_b64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.0)
    }

    /// Inverse de [`Self::to_b64`].
    pub fn parse_b64(s: &str) -> Result<Self, FingerprintError> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| FingerprintError)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| FingerprintError)?;
        Ok(Fingerprint(bytes))
    }

    /// Forme courte à comparer à l'œil : 64 premiers bits, `A1B2-C3D4-E5F6-0718`.
    pub fn short(&self) -> String {
        self.0[..8]
            .chunks(2)
            .map(|pair| format!("{:02X}{:02X}", pair[0], pair[1]))
            .collect::<Vec<_>>()
            .join("-")
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.short())
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_b64())
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Fingerprint::parse_b64(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empreinte_sha256_et_forme_courte() {
        // SHA-256("abc"), vecteur de test du NIST.
        let fp = Fingerprint::of_der(b"abc");
        assert_eq!(
            fp.as_bytes()[..8],
            [0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea]
        );
        assert_eq!(fp.short(), "BA78-16BF-8F01-CFEA");
        let b64 = fp.to_b64();
        assert_eq!(b64.len(), 43);
        assert!(
            b64.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{b64}"
        );
        assert_eq!(Fingerprint::parse_b64(&b64), Ok(fp));
        // Longueur fausse, remplissage ou alphabet standard : refusés.
        assert!(Fingerprint::parse_b64(&b64[..42]).is_err());
        assert!(Fingerprint::parse_b64(&format!("{b64}=")).is_err());
        assert!(Fingerprint::parse_b64("").is_err());
        assert!(Fingerprint::parse_b64("!!").is_err());
        // Sérialisé comme sa forme base64url ; `Debug` montre la forme courte.
        let json = serde_json::to_string(&fp).unwrap();
        assert_eq!(json, format!("\"{b64}\""));
        assert_eq!(serde_json::from_str::<Fingerprint>(&json).unwrap(), fp);
        assert_eq!(format!("{fp:?}"), "Fingerprint(BA78-16BF-8F01-CFEA)");
        assert_ne!(Fingerprint::of_der(b"abd"), fp);
    }
}
