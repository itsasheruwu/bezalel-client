use crate::state::{err, Result, Target};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::{Digest, Sha512};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Remote {
    pub source: String,
    pub project: String,
    pub version_id: String,
    pub version: String,
    pub filename: String,
    pub url: String,
    pub sha512: Option<String>,
    pub sha1: Option<String>,
    pub size: u64,
    pub deps: Vec<Edge>,
    pub client: bool,
    pub server: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub source: String,
    pub project: String,
    pub version: Option<String>,
    pub kind: String,
}
pub struct Providers {
    http: reqwest::Client,
}
impl Providers {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent("Bezalel/0.2.2 (https://github.com/itsasheruwu/bezalel-client)")
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(45))
                .build()
                .map_err(err)?,
        })
    }
    async fn get(&self, source: &str, path: &str, query: &[(String, String)]) -> Result<Value> {
        let base = match source {
            "Modrinth" => "https://api.modrinth.com/v2",
            "CurseForge" => "https://api.curseforge.com/v1",
            _ => return Err("Unsupported mod provider.".into()),
        };
        let mut request = self.http.get(format!("{base}{path}")).query(query);
        if source == "CurseForge" {
            request=request.header("x-api-key",std::env::var("BEZALEL_CURSEFORGE_API_KEY").map_err(|_|"CurseForge requires a provider API key (BEZALEL_CURSEFORGE_API_KEY). Choose Modrinth or supply the official JAR.")?);
        }
        let r = request
            .send()
            .await
            .map_err(|e| format!("{source} is unavailable or offline: {e}"))?;
        if r.status() == 429 {
            return Err(format!(
                "{source} rate limit reached. Retry after {} seconds.",
                r.headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("60")
            ));
        }
        if r.content_length().is_some_and(|n| n > 8 * 1024 * 1024) {
            return Err("Provider response exceeds metadata limit.".into());
        }
        let mut r = r
            .error_for_status()
            .map_err(|e| format!("{source} request failed: {e}"))?;
        let mut bytes = vec![];
        while let Some(chunk) = r.chunk().await.map_err(err)? {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err("Provider metadata too large.".into());
            }
            bytes.extend(chunk);
        }
        serde_json::from_slice(&bytes).map_err(err)
    }
    pub async fn identify(&self, bytes: &[u8], target: Target) -> Result<Option<Remote>> {
        let hash = format!("{:x}", Sha512::digest(bytes));
        // 404 is an ordinary unpublished/local mod; other errors remain visible.
        match self
            .get(
                "Modrinth",
                &format!("/version_file/{hash}"),
                &[("algorithm".into(), "sha512".into())],
            )
            .await
        {
            Ok(mut v) => {
                if !contains(&v["game_versions"], target.minecraft_version)
                    || !contains(&v["loaders"], target.loader)
                {
                    return Err("The publisher does not list this imported release for the selected Minecraft version and loader.".into());
                }
                if let Some(files) = v["files"].as_array_mut() {
                    for f in files {
                        f["primary"] =
                            Value::Bool(f["hashes"]["sha512"].as_str() == Some(hash.as_str()));
                    }
                }
                let remote = self.mr(&v).await?;
                verify(&remote, bytes)?;
                Ok(Some(remote))
            }
            Err(e) if e.contains("404") => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub async fn search(&self, id: &str, target: Target) -> Result<Vec<String>> {
        let facets = json!([
            [format!("categories:{}", target.loader)],
            [format!("versions:{}", target.minecraft_version)],
            ["project_type:mod"]
        ])
        .to_string();
        let v = self
            .get(
                "Modrinth",
                "/search",
                &[
                    ("query".into(), id.into()),
                    ("facets".into(), facets),
                    ("limit".into(), "8".into()),
                ],
            )
            .await?;
        let mut out = v["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                h["project_id"]
                    .as_str()
                    .map(|id| format!("Modrinth:{id} — {}", h["title"].as_str().unwrap_or(id)))
            })
            .collect::<Vec<_>>();
        if std::env::var_os("BEZALEL_CURSEFORGE_API_KEY").is_some() {
            let v = self
                .get(
                    "CurseForge",
                    "/mods/search",
                    &[
                        ("gameId".into(), "432".into()),
                        ("classId".into(), "6".into()),
                        ("searchFilter".into(), id.into()),
                        ("gameVersion".into(), target.minecraft_version.into()),
                        ("modLoaderType".into(), cf_loader(target).into()),
                        ("pageSize".into(), "8".into()),
                    ],
                )
                .await?;
            out.extend(v["data"].as_array().into_iter().flatten().filter_map(|h| {
                h["id"].as_u64().map(|id| {
                    format!(
                        "CurseForge:{id} — {}",
                        h["name"].as_str().unwrap_or("Project")
                    )
                })
            }));
        }
        Ok(out)
    }
    pub async fn versions(
        &self,
        source: &str,
        project: &str,
        exact: Option<&str>,
        target: Target,
    ) -> Result<Vec<Remote>> {
        if !project
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            || exact.is_some_and(|v| {
                !v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            })
        {
            return Err("Invalid provider project/version identity.".into());
        }
        let mut out = vec![];
        if source == "Modrinth" {
            let values = if let Some(id) = exact {
                vec![self.get(source, &format!("/version/{id}"), &[]).await?]
            } else {
                self.get(
                    source,
                    &format!("/project/{project}/version"),
                    &[
                        ("loaders".into(), json!([target.loader]).to_string()),
                        (
                            "game_versions".into(),
                            json!([target.minecraft_version]).to_string(),
                        ),
                    ],
                )
                .await?
                .as_array()
                .ok_or("Invalid Modrinth versions.")?
                .clone()
            };
            for v in values.into_iter().take(80) {
                if v["project_id"] != project
                    || !contains(&v["game_versions"], target.minecraft_version)
                    || !contains(&v["loaders"], target.loader)
                {
                    continue;
                }
                out.push(self.mr(&v).await?);
            }
        } else if source == "CurseForge" {
            let project_data = self.get(source, &format!("/mods/{project}"), &[]).await?;
            if project_data["data"]["allowModDistribution"] == false {
                return Err("This CurseForge publisher disallows automatic distribution. Download the official file yourself and supply it in the permission card.".into());
            }
            let values = if let Some(id) = exact {
                vec![self
                    .get(source, &format!("/mods/{project}/files/{id}"), &[])
                    .await?["data"]
                    .clone()]
            } else {
                self.get(
                    source,
                    &format!("/mods/{project}/files"),
                    &[
                        ("gameVersion".into(), target.minecraft_version.into()),
                        ("modLoaderType".into(), cf_loader(target).into()),
                        ("pageSize".into(), "50".into()),
                    ],
                )
                .await?["data"]
                    .as_array()
                    .ok_or("Invalid CurseForge files.")?
                    .clone()
            };
            for v in values {
                if !contains(&v["gameVersions"], target.minecraft_version)
                    || !contains(
                        &v["gameVersions"],
                        match target.loader {
                            "fabric" => "Fabric",
                            "forge" => "Forge",
                            _ => "NeoForge",
                        },
                    )
                {
                    continue;
                }
                let url=v["downloadUrl"].as_str().filter(|s|!s.is_empty()).ok_or("CurseForge automatic downloads are disabled for this file. Supply the official JAR; Bezalel will not construct a bypass URL.")?;
                let sha1 = v["hashes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|h| h["algo"] == 1)
                    .and_then(|h| h["value"].as_str())
                    .map(str::to_owned);
                out.push(Remote {
                    source: source.into(),
                    project: project.into(),
                    version_id: v["id"].to_string(),
                    version: v["displayName"].as_str().unwrap_or("").into(),
                    filename: v["fileName"].as_str().unwrap_or("").into(),
                    url: url.into(),
                    sha512: None,
                    sha1,
                    size: v["fileLength"].as_u64().unwrap_or(0),
                    client: true,
                    server: true,
                    deps: v["dependencies"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|d| Edge {
                            source: source.into(),
                            project: d["modId"].to_string(),
                            version: None,
                            kind: match d["relationType"].as_u64() {
                                Some(3) => "required",
                                Some(5) => "incompatible",
                                Some(6) => "embedded",
                                _ => "optional",
                            }
                            .into(),
                        })
                        .collect(),
                });
            }
        } else {
            return Err("Unknown source.".into());
        }
        Ok(out)
    }
    async fn mr(&self, v: &Value) -> Result<Remote> {
        let project = v["project_id"]
            .as_str()
            .ok_or("Missing project identity.")?;
        let p = self
            .get("Modrinth", &format!("/project/{project}"), &[])
            .await?;
        if p["project_type"] != "mod" {
            return Err("This project is not a Minecraft runtime mod.".into());
        }
        let files = v["files"].as_array().ok_or("No downloadable mod files.")?;
        let f = files
            .iter()
            .find(|f| f["primary"] == true)
            .or_else(|| {
                if files.len() == 1 {
                    files.first()
                } else {
                    None
                }
            })
            .ok_or("Ambiguous release assets; supply the official runtime JAR.")?;
        let mut deps = vec![];
        for d in v["dependencies"].as_array().into_iter().flatten() {
            let version = d["version_id"].as_str().map(str::to_owned);
            let project = if let Some(project) = d["project_id"].as_str() {
                Some(project.to_string())
            } else if let Some(version) = &version {
                let release = self
                    .get("Modrinth", &format!("/version/{version}"), &[])
                    .await?;
                Some(
                    release["project_id"]
                        .as_str()
                        .ok_or("Pinned dependency has no project identity.")?
                        .to_string(),
                )
            } else {
                None
            };
            if let Some(project) = project {
                deps.push(Edge {
                    source: "Modrinth".into(),
                    project,
                    version,
                    kind: d["dependency_type"].as_str().unwrap_or("optional").into(),
                });
            } else if d["dependency_type"] == "required" {
                return Err("Provider declares a required dependency without a project or release identity. Supply the official files explicitly.".into());
            }
        }
        Ok(Remote {
            source: "Modrinth".into(),
            project: project.into(),
            version_id: v["id"].as_str().unwrap_or("").into(),
            version: v["version_number"].as_str().unwrap_or("").into(),
            filename: f["filename"].as_str().unwrap_or("").into(),
            url: f["url"].as_str().unwrap_or("").into(),
            sha512: f["hashes"]["sha512"].as_str().map(str::to_owned),
            sha1: f["hashes"]["sha1"].as_str().map(str::to_owned),
            size: f["size"].as_u64().unwrap_or(0),
            client: p["client_side"] != "unsupported",
            server: p["server_side"] != "unsupported",
            deps,
        })
    }
    pub async fn download(&self, r: &Remote) -> Result<Vec<u8>> {
        validate_url(&r.url, &r.source)?;
        if r.size == 0 || r.size > 64 * 1024 * 1024 {
            return Err("Invalid or oversized mod download.".into());
        }
        let response = self
            .http
            .get(&r.url)
            .send()
            .await
            .map_err(|e| format!("Download unavailable/offline: {e}"))?;
        read_download(response, r).await
    }
}
/// Bound and verify every byte before it is eligible for staging or installation.
pub(crate) async fn read_download(mut response: reqwest::Response, r: &Remote) -> Result<Vec<u8>> {
    if response.status().as_u16() == 429 {
        return Err(format!(
            "{} download rate limit reached. Retry after {} seconds.",
            r.source,
            response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("60")
        ));
    }
    if response.status().is_redirection() {
        return Err("Provider redirected the download. Supply the official JAR instead.".into());
    }
    response = response.error_for_status().map_err(err)?;
    let mut bytes = vec![];
    while let Some(chunk) = response.chunk().await.map_err(err)? {
        if bytes.len() as u64 + chunk.len() as u64 > r.size {
            return Err("Download exceeds publisher size.".into());
        }
        bytes.extend(chunk);
    }
    verify(r, &bytes)?;
    Ok(bytes)
}

fn contains(v: &Value, s: &str) -> bool {
    v.as_array().is_some_and(|a| a.iter().any(|x| x == s))
}
fn cf_loader(t: Target) -> &'static str {
    match t.loader {
        "forge" => "1",
        "fabric" => "4",
        _ => "6",
    }
}
pub fn validate_url(url: &str, source: &str) -> Result<()> {
    let u = url::Url::parse(url).map_err(err)?;
    let allowed = match source {
        "Modrinth" => u.host_str() == Some("cdn.modrinth.com"),
        "CurseForge" => matches!(
            u.host_str(),
            Some("edge.forgecdn.net" | "mediafilez.forgecdn.net")
        ),
        "Fabric official" => u.host_str() == Some("maven.fabricmc.net"),
        _ => false,
    };
    if !allowed
        || u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port().is_some()
    {
        return Err("Download URL is outside the approved provider CDN.".into());
    }
    Ok(())
}
pub fn verify(r: &Remote, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 != r.size {
        return Err("Incomplete download; publisher size mismatch.".into());
    }
    if r.sha512.is_none() && r.sha1.is_none() {
        return Err(
            "Publisher supplied no supported checksum. Supply a verified official file.".into(),
        );
    }
    if r.sha512
        .as_ref()
        .is_some_and(|h| format!("{:x}", Sha512::digest(bytes)) != h.to_ascii_lowercase())
        || r.sha1
            .as_ref()
            .is_some_and(|h| format!("{:x}", Sha1::digest(bytes)) != h.to_ascii_lowercase())
    {
        return Err("Publisher checksum verification failed.".into());
    }
    Ok(())
}
