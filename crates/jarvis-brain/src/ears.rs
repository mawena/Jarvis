//! Registre des oreilles appairées (`~/.local/share/jarvis/ears.toml`, 0600,
//! écriture atomique). Seul le SHA-256 du jeton est stocké, comparé en temps constant.

use base64::Engine;
use jarvis_protocol::EarId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarRecord {
    pub id: EarId,
    pub name: String,
    pub platform: String,
    /// SHA-256 du jeton, en hexadécimal. Jamais le jeton lui-même.
    pub token_sha256: String,
    /// Oreille du PC (appairée par le socket Unix).
    pub local: bool,
    /// Baisse du son des lecteurs du PC à son réveil.
    #[serde(default = "default_duck")]
    pub duck: bool,
    /// Ajouté au rang de l'oreille par l'arbitre.
    #[serde(default)]
    pub bias: f32,
    /// Secondes depuis l'epoch Unix.
    pub paired_at: u64,
}

fn default_duck() -> bool {
    true
}

/// Contenu de `ears.toml` : une table `[[ear]]` par oreille.
#[derive(Default, Serialize, Deserialize)]
struct EarsFile {
    #[serde(default, rename = "ear")]
    ears: Vec<EarRecord>,
}

/// Début d'identifiant accepté au moins par [`EarRegistry::remove`] (`jarvis ears`
/// en montre 8 caractères).
pub const MIN_ID_PREFIX: usize = 4;

pub struct EarRegistry {
    path: PathBuf,
    ears: Vec<EarRecord>,
}

impl EarRegistry {
    /// Fichier absent : registre vide.
    pub fn load(path: &Path) -> io::Result<Self> {
        let ears = match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str::<EarsFile>(&text)
                    .map_err(|e| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("{} illisible : {e}", path.display()),
                        )
                    })?
                    .ears
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(EarRegistry {
            path: path.to_path_buf(),
            ears,
        })
    }

    /// `~/.local/share/jarvis/ears.toml`.
    pub fn default_path() -> PathBuf {
        match directories::ProjectDirs::from("", "", "jarvis") {
            Some(dirs) => dirs.data_dir().join("ears.toml"),
            None => PathBuf::from(".local/share/jarvis/ears.toml"),
        }
    }

    /// Oreille connue et jeton correct (comparaison en temps constant).
    pub fn authenticate(&self, id: &EarId, token: &str) -> Option<EarRecord> {
        let ear = self.ears.iter().find(|e| &e.id == id)?;
        let hash = token_hash(token);
        bool::from(hash.as_bytes().ct_eq(ear.token_sha256.as_bytes())).then(|| ear.clone())
    }

    /// Nouvelle oreille ; le jeton en clair n'est rendu qu'ici, une seule fois.
    pub fn add(
        &mut self,
        name: &str,
        platform: &str,
        local: bool,
    ) -> io::Result<(EarRecord, String)> {
        let (record, token) = new_record(name, platform, local)?;
        let mut ears = self.ears.clone();
        ears.push(record.clone());
        self.replace(ears)?;
        Ok((record, token))
    }

    /// Oreille locale : remplace celle du même nom (nouveau jeton).
    pub fn add_local(&mut self, name: &str) -> io::Result<(EarRecord, String)> {
        let (record, token) = new_record(name, std::env::consts::OS, true)?;
        let mut ears: Vec<EarRecord> = self
            .ears
            .iter()
            .filter(|e| !(e.local && e.name == name))
            .cloned()
            .collect();
        ears.push(record.clone());
        self.replace(ears)?;
        Ok((record, token))
    }

    /// Retire une oreille par son identifiant, son nom, ou le début de son identifiant
    /// (au moins [`MIN_ID_PREFIX`] caractères, comme la colonne Id de `jarvis ears`).
    /// Nom ou début d'identifiant ambigu : `InvalidInput`.
    pub fn remove(&mut self, name_or_id: &str) -> io::Result<Option<EarRecord>> {
        let index = match self.ears.iter().position(|e| e.id.as_str() == name_or_id) {
            Some(i) => i,
            None => match self.unique(|e| e.name == name_or_id) {
                Ok(Some(i)) => i,
                Ok(None) if name_or_id.len() >= MIN_ID_PREFIX => {
                    let prefix = name_or_id.to_ascii_lowercase();
                    match self.unique(|e| e.id.as_str().starts_with(&prefix)) {
                        Ok(Some(i)) => i,
                        Ok(None) => return Ok(None),
                        Err(()) => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                format!(
                                    "plusieurs oreilles ont un identifiant qui commence par \
                                     « {name_or_id} » : donnez-en plus"
                                ),
                            ));
                        }
                    }
                }
                Ok(None) => return Ok(None),
                Err(()) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "plusieurs oreilles s'appellent « {name_or_id} » : donnez son \
                             identifiant (colonne Id de « jarvis ears »)"
                        ),
                    ));
                }
            },
        };
        let mut ears = self.ears.clone();
        let removed = ears.remove(index);
        self.replace(ears)?;
        Ok(Some(removed))
    }

    /// Seule oreille qui vérifie `matches` : `Ok(None)` si aucune, `Err` si plusieurs.
    fn unique(&self, matches: impl Fn(&EarRecord) -> bool) -> Result<Option<usize>, ()> {
        let mut found = self.ears.iter().enumerate().filter(|(_, e)| matches(e));
        match (found.next(), found.next()) {
            (None, _) => Ok(None),
            (Some((i, _)), None) => Ok(Some(i)),
            (Some(_), Some(_)) => Err(()),
        }
    }

    pub fn list(&self) -> Vec<EarRecord> {
        self.ears.clone()
    }

    pub fn get(&self, id: &EarId) -> Option<EarRecord> {
        self.ears.iter().find(|e| &e.id == id).cloned()
    }

    /// Écrit la nouvelle liste, puis l'adopte (inchangée si l'écriture échoue).
    fn replace(&mut self, ears: Vec<EarRecord>) -> io::Result<()> {
        let file = EarsFile { ears };
        let text = toml::to_string(&file).map_err(io::Error::other)?;
        write_private(&self.path, text.as_bytes())?;
        self.ears = file.ears;
        Ok(())
    }
}

/// Fiche d'une nouvelle oreille et son jeton en clair.
fn new_record(name: &str, platform: &str, local: bool) -> io::Result<(EarRecord, String)> {
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes::<32>()?);
    let paired_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let record = EarRecord {
        id: EarId::new(hex(&random_bytes::<16>()?)),
        name: name.to_string(),
        platform: platform.to_string(),
        token_sha256: token_hash(&token),
        local,
        duck: true,
        bias: 0.0,
        paired_at,
    };
    Ok((record, token))
}

fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

pub(crate) fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(format!("aléa : {e}")))?;
    Ok(bytes)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Écriture atomique d'un fichier privé : dossier créé en 0700 s'il manque, fichier
/// temporaire 0600 à nom unique dans le même dossier, `fsync`, puis `rename`.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn registry() -> (tempfile::TempDir, EarRegistry) {
        let dir = tempfile::tempdir().unwrap();
        let reg = EarRegistry::load(&dir.path().join("jarvis/ears.toml")).unwrap();
        (dir, reg)
    }

    fn file(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("jarvis/ears.toml")
    }

    #[test]
    fn jeton_correct_accepte() {
        let (_dir, mut reg) = registry();
        let (ear, token) = reg.add("Téléphone", "android", false).unwrap();
        assert_eq!(ear.id.as_str().len(), 32);
        assert!(ear.id.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(ear.duck);
        assert_eq!(ear.bias, 0.0);
        assert!(!ear.local);
        assert_eq!(reg.authenticate(&ear.id, &token), Some(ear.clone()));
        assert_eq!(reg.get(&ear.id), Some(ear));
    }

    #[test]
    fn jeton_faux_refuse() {
        let (_dir, mut reg) = registry();
        let (ear, token) = reg.add("Téléphone", "android", false).unwrap();
        let (_, other) = reg.add("Salon", "linux", false).unwrap();
        assert_eq!(reg.authenticate(&ear.id, &other), None);
        assert_eq!(reg.authenticate(&ear.id, &token[1..]), None);
        assert_eq!(reg.authenticate(&ear.id, ""), None);
    }

    #[test]
    fn oreille_inconnue_refusee() {
        let (_dir, mut reg) = registry();
        let (_, token) = reg.add("Téléphone", "android", false).unwrap();
        assert_eq!(reg.authenticate(&EarId::new("0123"), &token), None);
        assert_eq!(reg.get(&EarId::new("0123")), None);
    }

    #[test]
    fn jeton_stocke_hache_jamais_en_clair() {
        let (dir, mut reg) = registry();
        let (ear, token) = reg.add("Téléphone", "android", false).unwrap();
        // 32 octets en base64url sans remplissage.
        assert_eq!(token.len(), 43);
        let text = std::fs::read_to_string(file(&dir)).unwrap();
        assert!(!text.contains(&token));
        assert!(text.contains(&ear.token_sha256));
        assert_eq!(ear.token_sha256.len(), 64);
        assert!(!format!("{ear:?}").contains(&token));
    }

    #[test]
    fn registre_relu_apres_redemarrage() {
        let (dir, mut reg) = registry();
        let (ear, token) = reg.add("Téléphone", "android", false).unwrap();
        let (local, _) = reg.add_local("PC").unwrap();
        let reread = EarRegistry::load(&file(&dir)).unwrap();
        assert_eq!(reread.list(), vec![ear.clone(), local]);
        assert_eq!(reread.authenticate(&ear.id, &token), Some(ear));
    }

    #[test]
    fn registre_absent_vide() {
        let (dir, reg) = registry();
        assert!(reg.list().is_empty());
        // Rien n'est écrit tant qu'aucune oreille n'est ajoutée.
        assert!(!file(&dir).exists());
    }

    #[test]
    fn reglages_par_oreille_relus() {
        let (dir, mut reg) = registry();
        reg.add("Téléphone", "android", false).unwrap();
        let text = std::fs::read_to_string(file(&dir)).unwrap();
        let text = text
            .replace("duck = true", "duck = false")
            .replace("bias = 0.0", "bias = 0.5");
        std::fs::write(file(&dir), text).unwrap();
        let ear = &EarRegistry::load(&file(&dir)).unwrap().list()[0];
        assert!(!ear.duck);
        assert_eq!(ear.bias, 0.5);
    }

    #[test]
    fn reglages_absents_valeurs_par_defaut() {
        let (dir, _) = registry();
        std::fs::create_dir_all(file(&dir).parent().unwrap()).unwrap();
        std::fs::write(
            file(&dir),
            "[[ear]]\nid = \"ab\"\nname = \"Salon\"\nplatform = \"linux\"\n\
             token_sha256 = \"00\"\nlocal = false\npaired_at = 1\n",
        )
        .unwrap();
        let ear = &EarRegistry::load(&file(&dir)).unwrap().list()[0];
        assert!(ear.duck);
        assert_eq!(ear.bias, 0.0);
    }

    #[test]
    fn registre_illisible_erreur() {
        let (dir, _) = registry();
        std::fs::create_dir_all(file(&dir).parent().unwrap()).unwrap();
        std::fs::write(file(&dir), "pas du toml [").unwrap();
        let err = EarRegistry::load(&file(&dir)).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn registre_prive_0600() {
        let (dir, mut reg) = registry();
        reg.add("Téléphone", "android", false).unwrap();
        reg.add("Salon", "linux", false).unwrap();
        let mode = std::fs::metadata(file(&dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let parent = std::fs::metadata(file(&dir).parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(parent & 0o777, 0o700);
        // Aucun fichier temporaire laissé.
        let names: Vec<_> = std::fs::read_dir(file(&dir).parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("ears.toml")]);
    }

    #[test]
    fn retrait_d_une_oreille() {
        let (dir, mut reg) = registry();
        let (a, token) = reg.add("Téléphone", "android", false).unwrap();
        let (b, _) = reg.add("Salon", "linux", false).unwrap();
        assert_eq!(reg.remove("Téléphone").unwrap(), Some(a.clone()));
        assert_eq!(reg.authenticate(&a.id, &token), None);
        assert_eq!(reg.remove(b.id.as_str()).unwrap(), Some(b));
        assert_eq!(reg.remove("Cuisine").unwrap(), None);
        assert!(EarRegistry::load(&file(&dir)).unwrap().list().is_empty());
    }

    #[test]
    fn retrait_par_nom_ambigu_refuse() {
        let (_dir, mut reg) = registry();
        reg.add("Téléphone", "android", false).unwrap();
        let (b, _) = reg.add("Téléphone", "android", false).unwrap();
        let err = reg.remove("Téléphone").err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(reg.list().len(), 2);
        assert_eq!(reg.remove(b.id.as_str()).unwrap(), Some(b));
    }

    #[test]
    fn retrait_par_debut_d_identifiant() {
        let (_dir, mut reg) = registry();
        let (a, _) = reg.add("Téléphone", "android", false).unwrap();
        let (b, _) = reg.add("Téléphone", "android", false).unwrap();
        let short = |r: &EarRecord| r.id.as_str()[..8].to_string();
        // Les 8 caractères montrés par `jarvis ears`, en majuscules ou non.
        assert_eq!(
            reg.remove(&short(&a).to_uppercase()).unwrap(),
            Some(a.clone())
        );
        assert_eq!(reg.list(), vec![b.clone()]);
        // Trop court, ou inconnu : rien.
        assert_eq!(reg.remove(&b.id.as_str()[..3]).unwrap(), None);
        assert_eq!(reg.remove("zzzzzzzz").unwrap(), None);
        assert_eq!(reg.remove(&short(&b)).unwrap(), Some(b));
    }

    #[test]
    fn retrait_par_debut_d_identifiant_ambigu_refuse() {
        let (_dir, mut reg) = registry();
        let (a, _) = reg.add("A", "linux", false).unwrap();
        let (b, _) = reg.add("B", "linux", false).unwrap();
        // Identifiants forcés au même début.
        let mut ears = reg.list();
        ears[0].id = EarId::new(format!("abcd{}", &a.id.as_str()[4..]));
        ears[1].id = EarId::new(format!("abcd{}", &b.id.as_str()[4..]));
        reg.replace(ears).unwrap();
        let err = reg.remove("abcd").err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(reg.list().len(), 2);
        // Un nom prime sur un début d'identifiant.
        assert_eq!(reg.remove("B").unwrap().map(|r| r.name), Some("B".into()));
    }

    #[test]
    fn oreille_locale_reappairee_remplace_le_jeton() {
        let (dir, mut reg) = registry();
        let (phone, _) = reg.add("PC", "android", false).unwrap();
        let (first, old) = reg.add_local("PC").unwrap();
        assert!(first.local);
        let (second, new) = reg.add_local("PC").unwrap();
        assert!(second.local);
        assert_ne!(old, new);
        assert_eq!(reg.authenticate(&first.id, &old), None);
        assert_eq!(reg.authenticate(&second.id, &new), Some(second.clone()));
        // L'oreille non locale du même nom reste.
        assert_eq!(reg.list(), vec![phone.clone(), second.clone()]);
        assert_eq!(
            EarRegistry::load(&file(&dir)).unwrap().list(),
            vec![phone, second]
        );
    }
}
