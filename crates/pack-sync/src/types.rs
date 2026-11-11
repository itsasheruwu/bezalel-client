use std::collections::HashSet;

use ring::signature::{ED25519, Ed25519KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::SyncError;

pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RELEASE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_RELEASE_FILES: usize = 128;
pub const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub const FABRIC_API_PATH: &str = "mods/fabric-api-0.110.0+1.21.1.jar";
pub const FABRIC_API_URL: &str = "https://maven.fabricmc.net/net/fabricmc/fabric-api/fabric-api/0.110.0+1.21.1/fabric-api-0.110.0+1.21.1.jar";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Editor,
    Player,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Editor => "editor",
            Self::Player => "player",
        }
    }

    pub fn parse(value: &str) -> Result<Self, SyncError> {
        match value {
            "owner" => Ok(Self::Owner),
            "editor" => Ok(Self::Editor),
            "player" => Ok(Self::Player),
            _ => Err("Unknown pack role".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Client,
    Server,
    Both,
}

impl Side {
    pub fn client(self) -> bool {
        self != Self::Server
    }

    pub fn server(self) -> bool {
        self != Self::Client
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub loader: String,
    pub minecraft_version: String,
    pub loader_version: String,
}

impl Target {
    pub fn validate(&self) -> Result<(), SyncError> {
        if !matches!(self.loader.as_str(), "fabric" | "forge" | "neoforge") {
            return Err("Unsupported mod loader".into());
        }
        if !version_component(&self.minecraft_version) || !version_component(&self.loader_version) {
            return Err("Invalid Minecraft or loader version".into());
        }
        Ok(())
    }
}

fn version_component(value: &str) -> bool {
    (1..=48).contains(&value.len())
        && value.as_bytes()[0].is_ascii_digit()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub target: Target,
    pub server_address: Option<String>,
    pub role: Role,
    pub active_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPack {
    pub name: String,
    pub target: Target,
    pub server_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackTrust {
    pub pack_id: Uuid,
    pub owner_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackMember {
    pub account: Account,
    pub role: Role,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewInvite {
    pub role: Role,
    pub expires_in_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub id: Uuid,
    pub code: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteRecord {
    pub id: Uuid,
    pub role: Role,
    pub expires_at: i64,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvitePreview {
    pub pack_id: Uuid,
    pub pack_name: String,
    pub owner: Account,
    pub target: Target,
    pub role: Role,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinPack {
    pub code: String,
    pub expected_owner_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub path: String,
    pub side: Side,
    pub bytes: u64,
    pub sha256: String,
}

impl Artifact {
    pub fn validate(&self) -> Result<(), SyncError> {
        validate_artifact_path(&self.path)?;
        validate_sha256(&self.sha256)?;
        if self.bytes == 0 || self.bytes > MAX_ARTIFACT_BYTES as u64 {
            return Err("Artifact exceeds the pack file size limit".into());
        }
        Ok(())
    }
}

pub fn fabric_api_artifact() -> Artifact {
    Artifact {
        path: FABRIC_API_PATH.into(),
        side: Side::Both,
        bytes: 2_384_492,
        sha256: "0890088b1a2b1d74f71aa4334aa7c221c1d530a786ba12811d1c7b859fd5b590".into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceChange {
    pub path: String,
    pub content: Option<String>,
}

pub fn validate_source_changes(changes: &[SourceChange]) -> Result<(), SyncError> {
    if changes.len() > MAX_RELEASE_FILES {
        return Err("Too many source changes".into());
    }
    let mut paths = HashSet::new();
    let mut bytes = 0;
    for change in changes {
        let is_java = change.path.starts_with("src/main/java/") && change.path.ends_with(".java");
        let is_resource = change.path.starts_with("src/main/resources/");
        if change.path.len() > 240
            || !(is_java || is_resource)
            || !change.path.split('/').all(portable_component)
            || !paths.insert(change.path.to_ascii_lowercase())
        {
            return Err("Invalid or duplicate source change path".into());
        }
        bytes += change.content.as_ref().map_or(0, String::len);
        if bytes > MAX_SOURCE_BYTES {
            return Err("Source changes exceed the size limit".into());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitMod {
    pub path: String,
    pub side: Side,
    pub jar_base64: String,
    pub changes: Vec<SourceChange>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitConfig {
    pub path: String,
    pub side: Side,
    pub text: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Submission {
    pub id: Uuid,
    pub author_id: Uuid,
    pub artifact: Artifact,
    pub changes: Vec<SourceChange>,
    pub notes: String,
    pub submitted_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateRequest {
    pub base_revision: Option<u64>,
    pub restore_revision: Option<u64>,
    pub submission_ids: Vec<Uuid>,
    pub remove_paths: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: Uuid,
    pub base_revision: Option<u64>,
    pub release: SignedRelease,
    pub remove_server_paths: Vec<String>,
}

impl Candidate {
    pub fn sign(
        id: Uuid,
        base_revision: Option<u64>,
        manifest: ReleaseManifest,
        remove_server_paths: Vec<String>,
        key: &Ed25519KeyPair,
    ) -> Result<Self, SyncError> {
        let mut candidate = Self {
            id,
            base_revision,
            release: SignedRelease {
                manifest,
                signature: String::new(),
            },
            remove_server_paths,
        };
        candidate.release.signature = hex::encode(key.sign(&candidate.signing_bytes()?).as_ref());
        Ok(candidate)
    }

    pub fn verify(&self, key: &[u8; 32]) -> Result<(), SyncError> {
        verify_signature(key, &self.signing_bytes()?, &self.release.signature)
    }

    fn signing_bytes(&self) -> Result<Vec<u8>, SyncError> {
        self.release.manifest.validate()?;
        if self.remove_server_paths.len() > MAX_RELEASE_FILES
            || self.base_revision.unwrap_or(0).checked_add(1)
                != Some(self.release.manifest.revision)
        {
            return Err("Invalid server bundle metadata".into());
        }
        for path in &self.remove_server_paths {
            validate_artifact_path(path)?;
        }
        let mut bytes = b"Bezalel pack candidate v1\n".to_vec();
        serde_json::to_writer(
            &mut bytes,
            &(
                self.id,
                self.base_revision,
                &self.release.manifest,
                &self.remove_server_paths,
            ),
        )
        .map_err(|error| error.to_string())?;
        Ok(bytes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishRequest {
    pub candidate_id: Uuid,
    pub server_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema: u8,
    pub pack_id: Uuid,
    pub owner_id: Uuid,
    pub revision: u64,
    pub target: Target,
    pub files: Vec<Artifact>,
    pub notes: String,
    pub created_at: i64,
}

impl ReleaseManifest {
    pub fn validate(&self) -> Result<(), SyncError> {
        self.target.validate()?;
        if self.schema != 1
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.notes.len() > 4096
            || self.files.len() > MAX_RELEASE_FILES
        {
            return Err("Invalid release metadata".into());
        }
        let mut paths = HashSet::new();
        let mut bytes = 0;
        for entry in &self.files {
            entry.validate()?;
            if !paths.insert(entry.path.to_ascii_lowercase()) {
                return Err("Release contains duplicate paths".into());
            }
            bytes += entry.bytes;
        }
        if bytes > MAX_RELEASE_BYTES {
            return Err("Release exceeds the pack size limit".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedRelease {
    pub manifest: ReleaseManifest,
    pub signature: String,
}

impl SignedRelease {
    pub fn sign(manifest: ReleaseManifest, key: &Ed25519KeyPair) -> Result<Self, SyncError> {
        manifest.validate()?;
        let bytes = manifest_bytes(&manifest)?;
        Ok(Self {
            signature: hex::encode(key.sign(&bytes).as_ref()),
            manifest,
        })
    }

    pub fn verify(&self, key: &[u8; 32], trust: &PackTrust) -> Result<(), SyncError> {
        self.manifest.validate()?;
        if self.manifest.pack_id != trust.pack_id || self.manifest.owner_id != trust.owner_id {
            return Err("This release does not match your trusted pack owner".into());
        }
        verify_signature(key, &manifest_bytes(&self.manifest)?, &self.signature)
    }
}

fn verify_signature(key: &[u8; 32], bytes: &[u8], signature: &str) -> Result<(), SyncError> {
    if signature.len() != 128 {
        return Err("Invalid release signature encoding".into());
    }
    let signature = hex::decode(signature).map_err(|_| "Invalid release signature encoding")?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(bytes, &signature)
        .map_err(|_| "Release signature verification failed".into())
}

fn manifest_bytes(manifest: &ReleaseManifest) -> Result<Vec<u8>, SyncError> {
    let mut bytes = b"Bezalel pack release v1\n".to_vec();
    serde_json::to_writer(&mut bytes, manifest).map_err(|error| error.to_string())?;
    Ok(bytes)
}

pub fn parse_public_key(value: &str) -> Result<[u8; 32], SyncError> {
    hex::decode(value.trim())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| "The sync public key must be 32 bytes in hexadecimal".into())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn validate_sha256(value: &str) -> Result<(), SyncError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Invalid SHA-256 digest".into());
    }
    Ok(())
}

pub fn validate_artifact_path(path: &str) -> Result<(), SyncError> {
    let file = path
        .strip_prefix("mods/")
        .filter(|name| name.ends_with(".jar"))
        .or_else(|| {
            path.strip_prefix("config/bezalel/").filter(|name| {
                [".toml", ".json", ".json5", ".properties", ".yaml", ".yml"]
                    .iter()
                    .any(|extension| name.ends_with(extension))
            })
        })
        .ok_or("Unsupported managed file path")?;
    if !portable_component(file) {
        return Err("Invalid managed file name".into());
    }
    Ok(())
}

fn portable_component(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 110
        || value.starts_with('.')
        || value.ends_with('.')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'))
    {
        return false;
    }
    let base = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return false;
    }
    !(base.len() == 4
        && (base.starts_with("COM") || base.starts_with("LPT"))
        && base.as_bytes()[3].is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair;

    fn fixture() -> (ReleaseManifest, Ed25519KeyPair, PackTrust) {
        let trust = PackTrust {
            pack_id: Uuid::new_v4(),
            owner_id: Uuid::new_v4(),
        };
        let manifest = ReleaseManifest {
            schema: 1,
            pack_id: trust.pack_id,
            owner_id: trust.owner_id,
            revision: 1,
            target: Target {
                loader: "fabric".into(),
                minecraft_version: "1.21.1".into(),
                loader_version: "0.18.4".into(),
            },
            files: vec![Artifact {
                path: "mods/lantern.jar".into(),
                side: Side::Both,
                bytes: 1,
                sha256: sha256_hex(b"a"),
            }],
            notes: "Initial release".into(),
            created_at: 1,
        };
        (
            manifest,
            Ed25519KeyPair::from_seed_unchecked(&[17; 32]).unwrap(),
            trust,
        )
    }

    #[test]
    fn signed_release_binds_metadata_and_owner() {
        let (manifest, key, mut trust) = fixture();
        let public_key = key.public_key().as_ref().try_into().unwrap();
        let mut signed = SignedRelease::sign(manifest, &key).unwrap();
        assert!(signed.verify(public_key, &trust).is_ok());
        trust.owner_id = Uuid::new_v4();
        assert!(signed.verify(public_key, &trust).is_err());
        trust.owner_id = signed.manifest.owner_id;
        signed.manifest.notes.push('!');
        assert!(signed.verify(public_key, &trust).is_err());
    }

    #[test]
    fn rejects_paths_that_escape_or_alias_on_windows() {
        for path in [
            "mods/../outside.jar",
            "mods/CON.jar",
            "mods/AUX.jar",
            "mods/alt:stream.jar",
            "mods/back\\slash.jar",
            "mods/foo.JAR",
            "config/bezalel/../anything.toml",
        ] {
            assert!(validate_artifact_path(path).is_err(), "{path}");
        }
        let (mut manifest, _, _) = fixture();
        let mut duplicate = manifest.files[0].clone();
        duplicate.path = "mods/Lantern.jar".into();
        manifest.files.push(duplicate);
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn candidate_signature_cannot_install_as_a_published_release() {
        let (manifest, key, trust) = fixture();
        let public_key = key.public_key().as_ref().try_into().unwrap();
        let mut candidate = Candidate::sign(
            Uuid::new_v4(),
            None,
            manifest,
            vec!["mods/old.jar".into()],
            &key,
        )
        .unwrap();
        assert!(candidate.verify(public_key).is_ok());
        assert!(candidate.release.verify(public_key, &trust).is_err());
        candidate.remove_server_paths[0] = "mods/different.jar".into();
        assert!(candidate.verify(public_key).is_err());
    }
}
