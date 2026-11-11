use std::{fs, io::Write, net::IpAddr, path::Path};

use reqwest::{Method, RequestBuilder, Response, Url};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;
use zip::write::SimpleFileOptions;

use crate::*;

pub struct SyncClient {
    base: Url,
    http: reqwest::Client,
    bearer: String,
    key: [u8; 32],
}

impl SyncClient {
    pub fn new(base_url: &str, bearer: &str, public_key_hex: &str) -> Result<Self, SyncError> {
        let mut base = Url::parse(base_url.trim()).map_err(|_| "Invalid sync service URL")?;
        let loopback = base.host_str().is_some_and(|host| {
            matches!(host, "localhost" | "[::1]")
                || host
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if !(base.scheme() == "https" || base.scheme() == "http" && loopback)
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("Use an HTTPS sync service URL or localhost for development".into());
        }
        let path = format!("{}/", base.path().trim_end_matches('/'));
        base.set_path(&path);
        if bearer.len() != 64 || !bearer.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Invalid Bezalel access token".into());
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .map_err(|_| "Could not initialize sync HTTP client")?;
        Ok(Self {
            base,
            http,
            bearer: bearer.into(),
            key: parse_public_key(public_key_hex)?,
        })
    }

    pub async fn me(&self) -> Result<Account, SyncError> {
        self.get("v1/me").await
    }

    pub async fn packs(&self) -> Result<Vec<Pack>, SyncError> {
        self.get("v1/packs").await
    }

    pub async fn create_pack(&self, new_pack: &NewPack) -> Result<Pack, SyncError> {
        self.post("v1/packs", new_pack).await
    }

    pub async fn preview_invite(&self, code: &str) -> Result<InvitePreview, SyncError> {
        self.post("v1/invites/preview", &serde_json::json!({ "code": code }))
            .await
    }

    pub async fn join(&self, join: &JoinPack) -> Result<Pack, SyncError> {
        self.post("v1/invites/join", join).await
    }

    pub async fn create_invite(&self, pack: Uuid, invite: &NewInvite) -> Result<Invite, SyncError> {
        self.post(&format!("v1/packs/{pack}/invites"), invite).await
    }

    pub async fn revoke_invite(&self, pack: Uuid, invite: Uuid) -> Result<(), SyncError> {
        self.delete(&format!("v1/packs/{pack}/invites/{invite}"))
            .await
    }

    pub async fn invites(&self, pack: Uuid) -> Result<Vec<InviteRecord>, SyncError> {
        self.get(&format!("v1/packs/{pack}/invites")).await
    }

    pub async fn members(&self, pack: Uuid) -> Result<Vec<PackMember>, SyncError> {
        self.get(&format!("v1/packs/{pack}/members")).await
    }

    pub async fn remove_member(&self, pack: Uuid, member: Uuid) -> Result<(), SyncError> {
        self.delete(&format!("v1/packs/{pack}/members/{member}"))
            .await
    }

    pub async fn submissions(&self, pack: Uuid) -> Result<Vec<Submission>, SyncError> {
        self.get(&format!("v1/packs/{pack}/submissions")).await
    }

    pub async fn submit(
        &self,
        pack: Uuid,
        submission: &SubmitMod,
    ) -> Result<Submission, SyncError> {
        self.post(&format!("v1/packs/{pack}/submissions"), submission)
            .await
    }

    pub async fn submit_config(
        &self,
        pack: Uuid,
        submission: &SubmitConfig,
    ) -> Result<Submission, SyncError> {
        self.post(&format!("v1/packs/{pack}/config"), submission)
            .await
    }

    pub async fn candidate(
        &self,
        pack: Uuid,
        candidate: &CandidateRequest,
    ) -> Result<Candidate, SyncError> {
        self.post(&format!("v1/packs/{pack}/candidates"), candidate)
            .await
    }

    pub async fn candidates(&self, pack: Uuid) -> Result<Vec<Candidate>, SyncError> {
        self.get(&format!("v1/packs/{pack}/candidates")).await
    }

    pub async fn publish(
        &self,
        pack: Uuid,
        request: &PublishRequest,
    ) -> Result<SignedRelease, SyncError> {
        self.post(&format!("v1/packs/{pack}/publish"), request)
            .await
    }

    pub async fn history(&self, pack: Uuid) -> Result<Vec<SignedRelease>, SyncError> {
        self.get(&format!("v1/packs/{pack}/releases")).await
    }

    pub async fn active(&self, pack: Uuid) -> Result<Option<SignedRelease>, SyncError> {
        self.get(&format!("v1/packs/{pack}/active")).await
    }

    pub async fn install(
        &self,
        release: &SignedRelease,
        trust: &PackTrust,
        lock: &PackLock,
        emit: impl Fn(u64, u64),
    ) -> Result<InstalledRelease, SyncError> {
        release.verify(&self.key, trust)?;
        if let Some(installed) = lock.installed()? {
            if installed.pack_id != trust.pack_id {
                return Err("This game directory belongs to a different pack".into());
            }
            if installed.revision > release.manifest.revision {
                return Err("The sync service returned an older revision".into());
            }
            if installed.revision == release.manifest.revision
                && installed.signature != release.signature
            {
                return Err("The signature changed for an installed revision".into());
            }
            if installed.signature == release.signature && lock.matches(release)? {
                emit(0, 0);
                return Ok(installed);
            }
        }
        let staged = lock.prepare()?;
        let files = release
            .manifest
            .files
            .iter()
            .filter(|file| file.side.client());
        let total = files.clone().map(|file| file.bytes).sum();
        let mut completed = 0;
        emit(completed, total);
        let staging: Result<(), SyncError> = async {
            for file in files {
                let bytes = match lock.cached(file)? {
                    Some(bytes) => bytes,
                    None => self.artifact(release.manifest.pack_id, file).await?,
                };
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(staged.join(&file.path))
                    .map_err(|error| error.to_string())?;
                output
                    .write_all(&bytes)
                    .map_err(|error| error.to_string())?;
                output.sync_all().map_err(|error| error.to_string())?;
                completed += file.bytes;
                emit(completed, total);
            }
            Ok(())
        }
        .await;
        if staging.is_err() {
            let _ = lock.discard_stage();
        }
        staging?;
        lock.activate(release)
    }

    pub async fn export_server(
        &self,
        candidate: &Candidate,
        output: &Path,
    ) -> Result<(), SyncError> {
        candidate.verify(&self.key)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .map_err(|error| format!("Could not create server bundle: {error}"))?;
        let result = self.write_server_zip(candidate, file).await;
        if result.is_err() {
            let _ = fs::remove_file(output);
        }
        result
    }

    async fn write_server_zip(
        &self,
        candidate: &Candidate,
        file: fs::File,
    ) -> Result<(), SyncError> {
        let manifest = &candidate.release.manifest;
        let mut archive = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        for entry in manifest.files.iter().filter(|file| file.side.server()) {
            let bytes = self.artifact(manifest.pack_id, entry).await?;
            archive
                .start_file(&entry.path, options)
                .map_err(|error| error.to_string())?;
            archive
                .write_all(&bytes)
                .map_err(|error| error.to_string())?;
        }
        archive
            .start_file("bezalel-release.json", options)
            .map_err(|error| error.to_string())?;
        serde_json::to_writer_pretty(&mut archive, candidate).map_err(|error| error.to_string())?;
        archive
            .start_file("remove-files.txt", options)
            .map_err(|error| error.to_string())?;
        for path in &candidate.remove_server_paths {
            writeln!(archive, "{path}").map_err(|error| error.to_string())?;
        }
        archive
            .start_file("README.txt", options)
            .map_err(|error| error.to_string())?;
        write!(archive, "Bezalel server update — release {}\n\nInstall Minecraft Java {} with {} {} using your host's loader installer.\nStop your server and back up its world and configuration.\nRemove the managed files listed in remove-files.txt, then copy mods/ and config/ into your server directory.\nRestart the server and test a join. In Bezalel, confirm readiness and publish the candidate.\n",
            manifest.revision, manifest.target.minecraft_version, manifest.target.loader, manifest.target.loader_version
        ).map_err(|error| error.to_string())?;
        archive
            .finish()
            .map_err(|error| error.to_string())?
            .sync_all()
            .map_err(|error| error.to_string())
    }

    async fn artifact(&self, pack: Uuid, entry: &Artifact) -> Result<Vec<u8>, SyncError> {
        let request = if *entry == fabric_api_artifact() {
            self.http.get(FABRIC_API_URL)
        } else {
            self.request(
                Method::GET,
                &format!("v1/packs/{pack}/artifacts/{}", entry.sha256),
            )?
        };
        let mut response = self.send(request).await?;
        if response
            .content_length()
            .is_some_and(|size| size != entry.bytes)
        {
            return Err(format!("Size check failed for {}", entry.path));
        }
        let expected = entry.bytes as usize;
        let mut bytes = Vec::with_capacity(expected);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Artifact download was interrupted")?
        {
            if chunk.len() > expected - bytes.len() {
                return Err(format!("Size check failed for {}", entry.path));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() != expected || sha256_hex(&bytes) != entry.sha256 {
            return Err(format!("Hash or size check failed for {}", entry.path));
        }
        Ok(bytes)
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, SyncError> {
        let url = self
            .base
            .join(path)
            .map_err(|_| "Invalid sync request URL")?;
        Ok(self.http.request(method, url).bearer_auth(&self.bearer))
    }

    async fn send(&self, request: RequestBuilder) -> Result<Response, SyncError> {
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                "Sync request timed out"
            } else {
                "Could not reach the sync service"
            }
        })?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        if response.content_length().is_some_and(|size| size <= 4096)
            && let Ok(error) = response.json::<serde_json::Value>().await
            && let Some(message) = error.get("message").and_then(serde_json::Value::as_str)
        {
            return Err(message.into());
        }
        Err(format!("Sync request failed ({status})"))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, SyncError> {
        self.send(self.request(Method::GET, path)?)
            .await?
            .json()
            .await
            .map_err(|_| "The sync service returned invalid data".into())
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, SyncError> {
        self.send(self.request(Method::POST, path)?.json(body))
            .await?
            .json()
            .await
            .map_err(|_| "The sync service returned invalid data".into())
    }

    async fn delete(&self, path: &str) -> Result<(), SyncError> {
        self.send(self.request(Method::DELETE, path)?).await?;
        Ok(())
    }
}
