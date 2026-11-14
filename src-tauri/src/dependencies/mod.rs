pub mod metadata;
pub mod permissions;
pub mod providers;
#[cfg(test)]
mod tests;
use crate::state::{self, err, Result, State, Target};
use metadata::{Mod, Requirement};
use permissions::{Item, Scope};
use providers::{Edge, Providers, Remote};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub pack_id: Uuid,
    pub root: String,
    pub filename: String,
    pub sha256: String,
    pub source: String,
    pub project: Option<String>,
    pub version: String,
    pub version_id: Option<String>,
    pub url: Option<String>,
    pub publisher_sha512: Option<String>,
    pub publisher_sha1: Option<String>,
    pub required_by: Vec<String>,
    pub dependencies: Vec<Requirement>,
    pub provider_dependencies: Vec<Edge>,
}
#[derive(Clone)]
pub struct Artifact {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub mods: Vec<Mod>,
    pub remote: Option<Remote>,
    pub reason: String,
}
pub struct Prepared {
    pub artifacts: Vec<Artifact>,
    pub records: Vec<Record>,
    pub inventory: BTreeMap<String, String>,
}
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn file_bytes(path: &Path) -> Result<Vec<u8>> {
    let m = fs::symlink_metadata(path).map_err(err)?;
    if !m.is_file() || state::is_link(&m) || m.len() > 64 * 1024 * 1024 {
        return Err("Choose a regular mod JAR below 64 MiB; redirected files are refused.".into());
    }
    fs::read(path).map_err(err)
}
pub fn filename(name: &str) -> Result<()> {
    bezalel_pack_sync::validate_artifact_path(&format!("mods/{name}"))?;
    if name.contains(['/', '\\', ':'])
        || !name.to_ascii_lowercase().ends_with(".jar")
        || name.starts_with('.')
    {
        return Err("Invalid runtime mod filename.".into());
    }
    Ok(())
}
pub fn inventory(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let mods = dir.join("mods");
    if !mods.exists() {
        return Ok(out);
    }
    state::ensure_directory(&mods)?;
    for e in fs::read_dir(mods).map_err(err)? {
        let e = e.map_err(err)?;
        let name = e
            .file_name()
            .into_string()
            .map_err(|_| "Invalid mod filename.")?;
        if name.to_ascii_lowercase().ends_with(".jar") {
            filename(&name)?;
            out.insert(name, bezalel_pack_sync::sha256_hex(&file_bytes(&e.path())?));
        }
    }
    Ok(out)
}
fn index(artifacts: &[Artifact], installed: &[Mod]) -> Result<BTreeMap<String, Mod>> {
    let mut out = BTreeMap::new();
    for m in installed
        .iter()
        .chain(artifacts.iter().flat_map(|a| a.mods.iter()))
    {
        if out.insert(m.id.clone(), m.clone()).is_some() {
            return Err(format!("Duplicate runtime mod identity {}. Remove the duplicate or choose which installed mod to replace.",m.id));
        }
    }
    Ok(out)
}
/// Validate every relationship again after the graph closes, including optional mods that are present.
pub fn validate(
    mods: &BTreeMap<String, Mod>,
    target: Target,
    side: &str,
) -> Result<Vec<(String, Requirement)>> {
    let mut missing = vec![];
    for m in mods.values() {
        if m.side != "*" && m.side != side {
            return Err(format!(
                "{} is {}-only and cannot run on this {} target.",
                m.id, m.side, side
            ));
        }
        for r in &m.requirements {
            if (r.side == "SERVER" && side == "client") || (r.side == "CLIENT" && side == "server")
            {
                continue;
            }
            let version = metadata::builtin(&r.id, target)
                .map(str::to_owned)
                .or_else(|| mods.get(&r.id).map(|m| m.version.clone()));
            match (r.kind.as_str(),version) {
                ("required",None)=>missing.push((m.id.clone(),r.clone())),
                ("incompatible",Some(v)) if metadata::satisfies(&v,r)?=>return Err(format!("{} conflicts with {} {} ({}).",m.id,r.id,v,r.ranges.join(" OR "))),
                ("required"|"optional",Some(v)) if !metadata::satisfies(&v,r)?=>return Err(format!("{} needs {} {}, but {} is selected. Existing files were left in place; choose a compatible file or remove the conflict.",m.id,r.id,r.ranges.join(" OR "),v)),
                _=>{}
            }
        }
    }
    Ok(missing)
}
fn sources() -> Vec<String> {
    let mut out = vec!["Modrinth".into()];
    if std::env::var_os("BEZALEL_CURSEFORGE_API_KEY").is_some() {
        out.push("CurseForge".into());
    }
    out
}
fn remote_key(r: &Remote) -> String {
    format!("{}:{}", r.source, r.project)
}
fn item(a: &Artifact) -> Item {
    Item {
        name: a.mods[0].id.clone(),
        version: a.mods[0].version.clone(),
        source: a.remote.as_ref().map_or("Local file".into(), |r| {
            format!(
                "{} project {} / release {}",
                r.source, r.project, r.version_id
            )
        }),
        reason: a.reason.clone(),
        filename: a.filename.clone(),
    }
}

/// Caller holds the target pack lock for planning, approval and commit. Bytes are immutable.
pub async fn prepare(
    state: &State,
    pack_id: Uuid,
    draft_id: Option<Uuid>,
    name: &str,
    bytes: Vec<u8>,
    replace: Option<&str>,
) -> Result<Prepared> {
    filename(name)?;
    let data = state.data()?;
    let pack = state::pack(&data, pack_id)?;
    if pack.owner_id.is_some() {
        return Err("Dependency installation is scoped to local packs. Shared packs require their signed release workflow.".into());
    }
    let target = state::target(&pack.target_id)?;
    let dir = state.game_dir(&data, pack_id);
    state::ensure_directory(&dir)?;
    let before = inventory(&dir)?;
    let mut installed = vec![];
    for existing in before.keys() {
        if Some(existing.as_str()) != replace {
            installed.extend(
                metadata::inspect(&file_bytes(&dir.join("mods").join(existing))?, target)
                    .map_err(|e| format!("Cannot inspect installed {existing}: {e}"))?,
            );
        }
    }
    let root_mods = metadata::inspect(&bytes, target)?;
    let root = root_mods[0].id.clone();
    let mut artifacts = vec![Artifact {
        filename: name.into(),
        bytes,
        mods: root_mods,
        remote: None,
        reason: "Requested mod".into(),
    }];
    let initial = index(&artifacts, &installed)?;
    validate(&initial, target, "client")?;
    let scope = Scope {
        pack_id,
        target_id: pack.target_id.clone(),
        action: "Search and stage".into(),
        sources: sources(),
    };
    let offline = data.settings.dependency_offline;
    let api = Providers::new()?;
    if !offline {
        permissions::ask(state,permissions::new_request(scope.clone(),draft_id,&root,&pack.name,"Look up publisher identities and runtime dependencies using provider APIs, then download candidate JARs to temporary staging for static inspection and checksum verification. No code is run. Installation requires its own approval.",vec![item(&artifacts[0])],vec![],vec![])).await?;
        artifacts[0].remote = api.identify(&artifacts[0].bytes, target).await?;
        if artifacts[0].remote.as_ref().is_some_and(|r| !r.client) {
            return Err("The publisher marks this imported mod as unsupported on clients.".into());
        }
    }
    let staging = state.root.join("dependency-staging");
    state::ensure_directory(&staging)?;
    let stage = Stage(staging.join(Uuid::new_v4().to_string()));
    state::ensure_directory(&stage.0)?;
    let mut resolved = HashSet::new();
    let mut provider_requirements: Vec<(String, Edge)> = vec![];
    for _ in 0..128 {
        let all = index(&artifacts, &installed)?;
        let missing = validate(&all, target, "client")?;
        provider_requirements.clear();
        for a in &artifacts {
            if let Some(r) = &a.remote {
                for edge in &r.deps {
                    provider_requirements.push((a.mods[0].id.clone(), edge.clone()));
                }
            }
        }
        let mut next = None;
        for (parent, e) in &provider_requirements {
            let selected = artifacts
                .iter()
                .filter_map(|a| a.remote.as_ref())
                .find(|r| r.source == e.source && r.project == e.project);
            if e.kind == "incompatible" && selected.is_some() {
                return Err(format!(
                    "{parent} declares an incompatible provider project {}.",
                    e.project
                ));
            }
            if e.kind != "required" {
                continue;
            }
            if let Some(r) = selected {
                if e.version.as_ref().is_some_and(|id| *id != r.version_id) {
                    return Err(format!("{parent} requires another release of {}. Resolve the version conflict explicitly.",e.project));
                }
                continue;
            }
            if resolved.contains(&format!("{}:{}", e.source, e.project)) {
                continue;
            }
            next = Some((parent.clone(), None, Some(e.clone())));
            break;
        }
        if next.is_none() {
            next = missing
                .iter()
                .find(|(_, r)| r.id == "fabric-api")
                .or(missing.first())
                .cloned()
                .map(|(p, r)| (p, Some(r), None));
        }
        let Some((parent, requirement, edge)) = next else {
            let changes = replace
                .map(|n| vec![format!("Replace owned mod {n} with {name}")])
                .unwrap_or_default();
            let mut used = artifacts
                .iter()
                .map(|a| {
                    a.remote
                        .as_ref()
                        .map_or("Local file".into(), |r| r.source.clone())
                })
                .collect::<Vec<_>>();
            used.sort();
            used.dedup();
            let install_scope = Scope {
                action: "Install".into(),
                sources: used,
                ..scope.clone()
            };
            let records = artifacts
                .iter()
                .map(|a| Record {
                    pack_id,
                    root: root.clone(),
                    filename: a.filename.clone(),
                    sha256: bezalel_pack_sync::sha256_hex(&a.bytes),
                    source: a
                        .remote
                        .as_ref()
                        .map_or("Local file".into(), |r| r.source.clone()),
                    project: a.remote.as_ref().map(|r| r.project.clone()),
                    version: a.mods[0].version.clone(),
                    version_id: a.remote.as_ref().map(|r| r.version_id.clone()),
                    url: a.remote.as_ref().map(|r| r.url.clone()),
                    publisher_sha512: a.remote.as_ref().and_then(|r| r.sha512.clone()),
                    publisher_sha1: a.remote.as_ref().and_then(|r| r.sha1.clone()),
                    dependencies: a.mods.iter().flat_map(|m| m.requirements.clone()).collect(),
                    provider_dependencies: a
                        .remote
                        .as_ref()
                        .map_or_else(Vec::new, |r| r.deps.clone()),
                    required_by: all
                        .values()
                        .filter(|m| {
                            m.requirements.iter().any(|r| {
                                r.kind == "required" && a.mods.iter().any(|m| m.id == r.id)
                            })
                        })
                        .map(|m| m.id.clone())
                        .chain(
                            provider_requirements
                                .iter()
                                .filter(|(_, e)| {
                                    a.remote.as_ref().is_some_and(|r| {
                                        r.source == e.source && r.project == e.project
                                    })
                                })
                                .map(|(p, _)| p.clone()),
                        )
                        .collect(),
                })
                .collect::<Vec<_>>();
            let warnings = all
                .values()
                .flat_map(|m| {
                    m.requirements
                        .iter()
                        .filter(|r| {
                            r.kind == "discouraged"
                                && all.get(&r.id).is_some_and(|other| {
                                    metadata::satisfies(&other.version, r).unwrap_or(false)
                                })
                        })
                        .map(|r| {
                            format!("{} discourages using {} {}", m.id, r.id, all[&r.id].version)
                        })
                })
                .collect::<Vec<_>>()
                .join("; ");
            // Include inventory hashes and exact artifacts in the approved immutable plan.
            let explanation=format!("Install these validated runtime JARs into {}. Optional missing mods are skipped. Existing mods stay in place except the replacements listed below. Plan {}",pack.name,bezalel_pack_sync::sha256_hex(&serde_json::to_vec(&(&before,&records)).map_err(err)?));
            let explanation = if warnings.is_empty() {
                explanation
            } else {
                format!("{explanation} Compatibility warnings: {warnings}.")
            };
            permissions::ask(
                state,
                permissions::new_request(
                    install_scope,
                    draft_id,
                    &root,
                    &pack.name,
                    &explanation,
                    artifacts.iter().map(item).collect(),
                    changes,
                    vec![],
                ),
            )
            .await?;
            if inventory(&dir)? != before {
                return Err("The modpack changed while approval was pending. Retry to approve a fresh plan.".into());
            }
            return Ok(Prepared {
                artifacts,
                records,
                inventory: before,
            });
        };
        if artifacts.len() > 64 {
            return Err("Dependency graph exceeds 64 downloadable mods.".into());
        }
        if offline {
            return Err(format!("Offline mode: {parent} requires {}. Import the official dependency file first or turn off offline mode.",requirement.as_ref().map_or("a provider dependency",|r|&r.id)));
        }
        let mut accepted = None;
        let (source, project, exact, local): (String, String, Option<String>, Option<PathBuf>) =
            if let Some(e) = edge {
                (e.source, e.project, e.version, None)
            } else {
                let r = requirement.as_ref().unwrap();
                if r.id == "fabric-api" {
                    ("Modrinth".into(), "P7dR8mSH".into(), None, None)
                } else {
                    let candidate =
                        discover(&api, r, &parent, target, &artifacts, &installed).await?;
                    let remote = candidate.remote.as_ref().unwrap();
                    let identity = (
                        remote.source.clone(),
                        remote.project.clone(),
                        Some(remote.version_id.clone()),
                        None,
                    );
                    accepted = Some(candidate);
                    identity
                }
            };
        if let Some(path) = local {
            if !path.is_absolute() {
                return Err("Select an absolute path to a local JAR.".into());
            }
            let bytes = file_bytes(&path)?;
            let mods = metadata::inspect(&bytes, target)?;
            let filename = path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("Invalid mod filename.")?
                .into();
            let remote = api.identify(&bytes, target).await?;
            if remote.as_ref().is_some_and(|r| !r.client) {
                return Err(
                    "The publisher marks this dependency as unsupported on clients.".into(),
                );
            }
            accepted = Some(Artifact {
                filename,
                bytes,
                mods,
                remote,
                reason: format!("Required by {parent}"),
            });
        } else if accepted.is_none() {
            let versions_result = api
                .versions(&source, &project, exact.as_deref(), target)
                .await;
            let versions = match versions_result {
                Ok(versions) => versions,
                Err(error) => {
                    let response=permissions::ask(state,permissions::new_request(Scope{action:"Provide file".into(),..scope.clone()},draft_id,&root,&pack.name,&format!("{parent} requires {source} project {project}. {error} Supply a file downloaded from that official project, or deny to stop. Its runtime metadata will still be validated; this does not bypass provider download restrictions."),vec![],vec![],vec!["Provide local JAR".into()])).await?;
                    let path = response
                        .file
                        .ok_or("Select the publisher-downloaded runtime JAR.")?;
                    let bytes = file_bytes(&path)?;
                    let mods = metadata::inspect(&bytes, target)?;
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .ok_or("Invalid filename.")?
                        .to_string();
                    accepted=Some(Artifact{filename,bytes,mods,remote:None,reason:format!("Required by {parent}; user-supplied file for {source} project {project}")});
                    resolved.insert(format!("{source}:{project}"));
                    vec![]
                }
            };
            for r in versions.into_iter().take(24) {
                if !r.client {
                    continue;
                }
                let bytes = api.download(&r).await?;
                let mods = metadata::inspect(&bytes, target)?;
                if requirement.as_ref().is_some_and(|req| {
                    !mods.iter().any(|m| {
                        m.id == req.id && metadata::satisfies(&m.version, req).unwrap_or(false)
                    })
                }) {
                    continue;
                }
                // Do not download duplicates when a provider dependency is already installed locally.
                if mods
                    .iter()
                    .all(|m| all.get(&m.id).is_some_and(|old| old.version == m.version))
                {
                    resolved.insert(remote_key(&r));
                    accepted = None;
                    break;
                }
                let candidate = Artifact {
                    filename: r.filename.clone(),
                    bytes,
                    mods,
                    remote: Some(r),
                    reason: format!("Required by {parent}"),
                };
                let mut trial = artifacts.clone();
                trial.push(candidate.clone());
                if index(&trial, &installed)
                    .and_then(|m| validate(&m, target, "client"))
                    .is_ok()
                {
                    accepted = Some(candidate);
                    break;
                }
            }
        }
        if let Some(a) = accepted {
            filename(&a.filename)?;
            if requirement.as_ref().is_some_and(|r| {
                !a.mods
                    .iter()
                    .any(|m| m.id == r.id && metadata::satisfies(&m.version, r).unwrap_or(false))
            }) {
                return Err(
                    "Selected file does not provide the required mod identity and version.".into(),
                );
            }
            if before.contains_key(&a.filename)
                || artifacts
                    .iter()
                    .any(|old| old.filename.eq_ignore_ascii_case(&a.filename))
            {
                return Err(
                    "Dependency filename collides with an existing file. No files changed.".into(),
                );
            }
            fs::write(stage.0.join(&a.filename), &a.bytes).map_err(err)?;
            artifacts.push(a);
        } else if !resolved.contains(&format!("{source}:{project}")) {
            return Err(format!("No compatible release of {source} project {project} satisfies {parent}. Choose another file/version and retry; existing files are unchanged."));
        }
    }
    Err("Dependency resolution exceeded the bounded graph limit (possible provider metadata cycle).".into())
}

/// Search labels are never sufficient: inspect publisher-verified runtime JARs
/// and select only a unique project that provides the requested identity/range.
async fn discover(
    api: &Providers,
    requirement: &Requirement,
    parent: &str,
    target: Target,
    artifacts: &[Artifact],
    installed: &[Mod],
) -> Result<Artifact> {
    let mut verified = vec![];
    let mut issues = vec![];
    for result in api
        .search(&requirement.id, target)
        .await?
        .into_iter()
        .take(8)
    {
        let identity = result.split(" \u{2014} ").next().unwrap_or(&result);
        let Some((source, project)) = identity.split_once(':') else {
            continue;
        };
        let releases = match api.versions(source, project, None, target).await {
            Ok(releases) => releases,
            Err(error) => {
                if error.contains("rate limit") {
                    return Err(error);
                }
                issues.push(error);
                continue;
            }
        };
        for remote in releases.into_iter().take(4) {
            if !remote.client {
                continue;
            }
            let bytes = match api.download(&remote).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    if error.contains("rate limit") {
                        return Err(error);
                    }
                    issues.push(error);
                    break;
                }
            };
            let mods = match metadata::inspect(&bytes, target) {
                Ok(mods) => mods,
                Err(_) => continue,
            };
            if !mods.iter().any(|m| {
                m.id == requirement.id
                    && metadata::satisfies(&m.version, requirement).unwrap_or(false)
            }) {
                continue;
            }
            let candidate = Artifact {
                filename: remote.filename.clone(),
                bytes,
                mods,
                remote: Some(remote),
                reason: format!("Required by {parent}"),
            };
            let mut trial = artifacts.to_vec();
            trial.push(candidate.clone());
            if index(&trial, installed)
                .and_then(|m| validate(&m, target, "client"))
                .is_ok()
            {
                verified.push(candidate);
                break;
            }
        }
    }
    unique_match(verified, &requirement.id).map_err(|error| {
        if issues.is_empty() {
            error
        } else {
            format!("{error} Provider details: {}", issues.join("; "))
        }
    })
}
fn unique_match(mut matches: Vec<Artifact>, id: &str) -> Result<Artifact> {
    // Prefer the actual requested mod over adapters that only advertise its ID
    // as a bundled component or `provides` alias (e.g. a compatibility layer).
    if matches
        .iter()
        .any(|a| a.mods.first().is_some_and(|m| m.id == id))
    {
        matches.retain(|a| a.mods.first().is_some_and(|m| m.id == id));
    }
    match matches.len() {
        1=>Ok(matches.remove(0)),
        0=>Err(format!("No verified compatible project provides {id}. Check the provider or supply its official JAR; no files were installed.")),
        _=>Err(format!("Multiple verified projects provide {id}: {}. Research the official project identity before retrying; no files were installed.",matches.iter().filter_map(|a|a.remote.as_ref()).map(remote_key).collect::<Vec<_>>().join(", ")))
    }
}

pub async fn import(state: &State, id: Uuid, draft: Option<Uuid>, source: &Path) -> Result<String> {
    let _job = state.acquire(format!("pack:{id}"))?;
    let data = state.data()?;
    let dir = state.game_dir(&data, id);
    let _lock = bezalel_pack_sync::PackLock::acquire(&dir)?;
    crate::minecraft::game_process::ensure_idle(&dir)?;
    let name = source
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Invalid filename.")?;
    let prepared = prepare(state, id, draft, name, file_bytes(source)?, None).await?;
    commit_import(state, id, &dir, prepared)?;
    Ok(name.into())
}
pub fn commit_import(state: &State, id: Uuid, dir: &Path, p: Prepared) -> Result<()> {
    if inventory(dir)? != p.inventory {
        return Err("Modpack changed; request a fresh install plan.".into());
    }
    crate::minecraft::game_process::ensure_idle(dir)?;
    let mut changes = crate::mod_installation::JarChanges::default();
    for a in &p.artifacts {
        changes.create(&dir.join("mods").join(&a.filename), &a.bytes)?;
    }
    state.update(|d| {
        state::pack(d, id)?;
        d.dependency_records.extend(p.records);
        Ok(())
    })?;
    changes.committed = true;
    Ok(())
}

pub async fn agent_install(state: &State, draft_id: Uuid, source: &Path) -> Result<String> {
    let draft = state::draft(&state.data()?, draft_id)?;
    let id = draft
        .pack_id
        .ok_or("Select a local modpack before installing runtime dependencies.")?;
    let _job = state.acquire(format!("pack:{id}"))?;
    let data = state.data()?;
    let dir = state.game_dir(&data, id);
    let _lock = bezalel_pack_sync::PackLock::acquire(&dir)?;
    crate::minecraft::game_process::ensure_idle(&dir)?;
    let name = source
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("Invalid mod filename.")?;
    let bytes = file_bytes(source)?;
    let prepared = prepare(
        state,
        id,
        Some(draft_id),
        name,
        bytes.clone(),
        draft.installed_mod.as_ref().map(|m| m.filename.as_str()),
    )
    .await?;
    crate::mod_installation::install_bundle(
        state,
        &draft,
        id,
        name,
        &bytes,
        &bezalel_pack_sync::sha256_hex(&bytes),
        Some(prepared),
    )?;
    Ok(format!("Installed {name} and its approved runtime dependencies in {}. Dependency versions, publisher identities, hashes and relationships were recorded.",state::pack(&data,id)?.name))
}

pub fn agent_inventory(state: &State, draft_id: Uuid) -> Result<String> {
    let data = state.data()?;
    let draft = state::draft(&data, draft_id)?;
    let id = draft.pack_id.ok_or("Select a modpack first.")?;
    let pack = state::pack(&data, id)?;
    let target = state::target(&pack.target_id)?;
    let dir = state.game_dir(&data, id);
    let mut files = vec![];
    for name in inventory(&dir)?.keys() {
        files.push(serde_json::json!({"filename":name,"mods":metadata::inspect(&file_bytes(&dir.join("mods").join(name))?,target)?}));
    }
    serde_json::to_string(&serde_json::json!({"packId":id,"target":pack.target_id,"files":files,"provenance":data.dependency_records.iter().filter(|r|r.pack_id==id).collect::<Vec<_>>()})).map_err(err)
}
