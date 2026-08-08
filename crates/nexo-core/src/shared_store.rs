//! The account store shared with Nexo Mod.
//!
//! One encrypted file, read and written by both halves of the project, so an
//! account added in the launcher shows up in the in-game switcher and the
//! other way round. Read `Mod/docs/SHARED-ACCOUNT-STORE.md` before changing
//! anything here — the format has to stay byte-compatible with
//! `AccountStore.java`, and a mismatch does not raise an error, it fails the
//! GCM tag check and looks like a corrupt store.
//!
//! Layout:
//!
//! ```text
//! 0   8 bytes   "NEXOACC" + version byte
//! 8   12 bytes  GCM nonce, fresh per write
//! 20  ...       AES-256-GCM ciphertext with the 128-bit tag appended
//! ```
//!
//! The header is also the GCM additional data, so a tampered header fails the
//! tag check rather than decrypting.

use crate::auth::{Account, SkinModel};
use crate::error::{Error, IoContext, Result};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// File marker and format version. The trailing byte must be bumped in both
/// languages together for any breaking change.
const HEADER: [u8; 8] = [b'N', b'E', b'X', b'O', b'A', b'C', b'C', 2];
const NONCE_BYTES: usize = 12;

/// Gson serialises Java record components by name, so these field names are
/// part of the wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAccount {
    name: String,
    /// Dashed, because Java parses it with `UUID.fromString`, which rejects
    /// the dashless form the Minecraft profile API returns.
    uuid: String,
    minecraft_access_token: String,
    microsoft_refresh_token: String,
    expires_at_epoch_second: u64,
    /// The Mod's offline-account concept. The launcher has no use for it but
    /// must round-trip it, or offline accounts silently lose the flag.
    #[serde(default)]
    offline: bool,

    // Cosmetics. Absent in stores written by older builds, hence optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skin_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skin_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cape_url: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredData {
    #[serde(default)]
    accounts: Vec<StoredAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_uuid: Option<String>,
}

/// What the launcher works with: accounts plus which one is active.
#[derive(Debug, Default, Clone)]
pub struct Contents {
    pub accounts: Vec<Account>,
    pub active: Option<String>,
    /// Offline flags, keyed by dashless uuid. The launcher has no concept of
    /// offline accounts but must carry the flag through, or the Mod's offline
    /// accounts silently lose it on the next launcher write.
    pub offline: std::collections::HashSet<String>,
}

impl Contents {
    pub fn is_offline(&self, uuid: &str) -> bool {
        self.offline.contains(uuid)
    }
}

#[derive(Debug, Clone)]
pub struct SharedStore {
    path: PathBuf,
}

impl SharedStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Reads and decrypts. An absent file is an empty store, not an error.
    ///
    /// A file that exists but cannot be decrypted *is* an error: it may have
    /// come from another machine, and silently treating it as empty would
    /// invite overwriting somebody's accounts.
    pub async fn load(&self) -> Result<Contents> {
        if !self.path.exists() {
            return Ok(Contents::default());
        }

        let raw = tokio::fs::read(&self.path).await.ctx(&self.path)?;
        let plaintext = decrypt(&raw, &key()?)?;
        let data: StoredData = serde_json::from_slice(&plaintext)?;

        let mut offline = std::collections::HashSet::new();
        let mut accounts = Vec::new();

        for stored in data.accounts {
            let uuid = undash(&stored.uuid);
            if stored.offline {
                offline.insert(uuid.clone());
            }
            accounts.push(Account {
                uuid,
                username: stored.name,
                access_token: stored.minecraft_access_token,
                refresh_token: stored.microsoft_refresh_token,
                expires_at: stored.expires_at_epoch_second,
                skin_url: stored.skin_url,
                skin_model: match stored.skin_model.as_deref() {
                    Some(model) if model.eq_ignore_ascii_case("SLIM") => SkinModel::Slim,
                    _ => SkinModel::Classic,
                },
                cape_url: stored.cape_url,
            });
        }

        Ok(Contents {
            accounts,
            active: data.active_uuid.map(|uuid| undash(&uuid)),
            offline,
        })
    }

    /// Encrypts and writes, replacing the file atomically.
    pub async fn save(&self, contents: &Contents) -> Result<()> {
        let data = StoredData {
            accounts: contents
                .accounts
                .iter()
                .map(|account| StoredAccount {
                    name: account.username.clone(),
                    uuid: dash(&account.uuid),
                    minecraft_access_token: account.access_token.clone(),
                    microsoft_refresh_token: account.refresh_token.clone(),
                    expires_at_epoch_second: account.expires_at,
                    offline: contents.is_offline(&account.uuid),
                    skin_url: account.skin_url.clone(),
                    skin_model: Some(
                        match account.skin_model {
                            SkinModel::Classic => "CLASSIC",
                            SkinModel::Slim => "SLIM",
                        }
                        .to_string(),
                    ),
                    cape_url: account.cape_url.clone(),
                })
                .collect(),
            active_uuid: contents.active.as_deref().map(dash),
        };

        let plaintext = serde_json::to_vec(&data)?;
        let encrypted = encrypt(&plaintext, &key()?)?;

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.ctx(parent)?;
        }

        // Write-then-rename: the game may be reading this file while the
        // launcher writes it, and a torn read would look like corruption.
        let temp = self.path.with_extension("dat.tmp");
        tokio::fs::write(&temp, &encrypted).await.ctx(&temp)?;
        tokio::fs::rename(&temp, &self.path).await.ctx(&self.path)?;

        restrict(&self.path).await
    }
}

/// This machine's key, or an error saying why there isn't one.
fn key() -> Result<Aes256Gcm> {
    let derived = crate::hwkey::derive().ok_or_else(|| {
        Error::invalid(
            "no hardware identifiers are readable on this machine, so the account \
             store cannot be encrypted or decrypted",
        )
    })?;

    let key = Key::<Aes256Gcm>::try_from(&derived.key()[..])
        .map_err(|_| Error::invalid("derived key was not 32 bytes"))?;
    Ok(Aes256Gcm::new(&key))
}

fn encrypt(plaintext: &[u8], cipher: &Aes256Gcm) -> Result<Vec<u8>> {
    // v4 UUIDs come from the OS CSPRNG, which is what a GCM nonce needs; only
    // uniqueness per key matters, and 12 of those 16 bytes give that.
    let random = uuid::Uuid::new_v4();
    let nonce_bytes: [u8; NONCE_BYTES] = random.as_bytes()[..NONCE_BYTES]
        .try_into()
        .expect("uuid is 16 bytes");
    let nonce = Nonce::try_from(&nonce_bytes[..])
        .map_err(|_| Error::invalid("nonce was not 12 bytes"))?;

    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &HEADER,
            },
        )
        .map_err(|_| Error::invalid("could not encrypt the account store"))?;

    let mut out = Vec::with_capacity(HEADER.len() + NONCE_BYTES + ciphertext.len());
    out.extend_from_slice(&HEADER);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt(stored: &[u8], cipher: &Aes256Gcm) -> Result<Vec<u8>> {
    if stored.len() < HEADER.len() + NONCE_BYTES || stored[..HEADER.len()] != HEADER {
        return Err(Error::invalid(
            "that is not a current-format Nexo account store",
        ));
    }

    let nonce = Nonce::try_from(&stored[HEADER.len()..HEADER.len() + NONCE_BYTES])
        .map_err(|_| Error::invalid("stored nonce was not 12 bytes"))?;
    let ciphertext = &stored[HEADER.len() + NONCE_BYTES..];

    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad: &HEADER,
            },
        )
        .map_err(|_| {
            Error::invalid(
                "the account store could not be decrypted — it was written on a \
                 different machine, or this machine's hardware changed",
            )
        })
}

/// `0123…` → `01234567-89ab-…`. Java parses with `UUID.fromString`, which
/// throws on the dashless form, so writing it dashless would make the Mod
/// fail to load the store at all.
fn dash(uuid: &str) -> String {
    let clean: String = uuid.chars().filter(|c| *c != '-').collect();
    if clean.len() != 32 {
        // Not a UUID; pass it through rather than corrupting it.
        return uuid.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &clean[0..8],
        &clean[8..12],
        &clean[12..16],
        &clean[16..20],
        &clean[20..32]
    )
}

/// The dashless form the Minecraft profile API uses, which is what the rest
/// of this crate keys accounts by.
fn undash(uuid: &str) -> String {
    uuid.chars().filter(|c| *c != '-').collect()
}

#[cfg(unix)]
async fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .await
        .ctx(path)
}

#[cfg(not(unix))]
async fn restrict(_path: &Path) -> Result<()> {
    // Windows inherits the user-profile ACL, already owner-scoped.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(uuid: &str, name: &str) -> Account {
        Account {
            uuid: uuid.into(),
            username: name.into(),
            access_token: "token".into(),
            refresh_token: "refresh".into(),
            expires_at: 1_800_000_000,
            skin_url: Some("https://example.invalid/skin.png".into()),
            skin_model: SkinModel::Slim,
            cape_url: None,
        }
    }

    #[test]
    fn uuids_round_trip_between_the_two_forms() {
        let dashless = "069a79f444e94726a5befca90e38aaf5";
        let dashed = "069a79f4-44e9-4726-a5be-fca90e38aaf5";

        assert_eq!(dash(dashless), dashed);
        assert_eq!(undash(dashed), dashless);
        // Already-dashed input must not gain a second set.
        assert_eq!(dash(dashed), dashed);
    }

    #[test]
    fn non_uuid_strings_pass_through_unchanged() {
        assert_eq!(dash("offline-player"), "offline-player");
    }

    #[test]
    fn encrypted_bytes_carry_the_header_and_nonce() {
        let Ok(cipher) = key() else {
            // No hardware identifiers in this environment; nothing to assert.
            return;
        };

        let encrypted = encrypt(b"hello", &cipher).unwrap();
        assert_eq!(&encrypted[..8], &HEADER);
        // Header, nonce, ciphertext, and a 16-byte tag.
        assert_eq!(encrypted.len(), 8 + 12 + 5 + 16);

        assert_eq!(decrypt(&encrypted, &cipher).unwrap(), b"hello");
    }

    #[test]
    fn a_tampered_header_fails_the_tag_check() {
        let Ok(cipher) = key() else { return };

        let mut encrypted = encrypt(b"hello", &cipher).unwrap();
        // Flip the version byte, which is also authenticated data.
        encrypted[7] = 3;
        assert!(decrypt(&encrypted, &cipher).is_err());
    }

    #[tokio::test]
    async fn accounts_survive_a_save_and_load() {
        if key().is_err() {
            return;
        }

        let path = std::env::temp_dir().join(format!("nexo-shared-{}.dat", uuid::Uuid::new_v4()));
        let store = SharedStore::new(&path);

        let mut contents = Contents {
            accounts: vec![
                account("069a79f444e94726a5befca90e38aaf5", "Alpha"),
                account("853c80ef3c3749fdaa49938b674adae6", "Beta"),
            ],
            active: Some("853c80ef3c3749fdaa49938b674adae6".into()),
            ..Contents::default()
        };
        // The Mod's flag must survive a launcher write.
        contents
            .offline
            .insert("069a79f444e94726a5befca90e38aaf5".into());

        store.save(&contents).await.unwrap();
        let loaded = store.load().await.unwrap();

        assert_eq!(loaded.accounts.len(), 2);
        assert_eq!(loaded.active.as_deref(), Some("853c80ef3c3749fdaa49938b674adae6"));
        assert_eq!(loaded.accounts[0].username, "Alpha");
        assert_eq!(loaded.accounts[0].skin_model, SkinModel::Slim);
        assert!(loaded.is_offline("069a79f444e94726a5befca90e38aaf5"));
        assert!(!loaded.is_offline("853c80ef3c3749fdaa49938b674adae6"));

        tokio::fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn a_missing_file_is_an_empty_store() {
        let path = std::env::temp_dir().join(format!("nexo-absent-{}.dat", uuid::Uuid::new_v4()));
        let loaded = SharedStore::new(&path).load().await.unwrap();
        assert!(loaded.accounts.is_empty());
        assert!(loaded.active.is_none());
    }
}
