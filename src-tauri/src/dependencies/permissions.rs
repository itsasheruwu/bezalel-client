//! Permissions live in the backend. A response consumes exactly one immutable request.
use crate::state::{err, Result, State};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub pack_id: Uuid,
    pub target_id: String,
    pub action: String,
    pub sources: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub id: Uuid,
    pub scope: Scope,
    pub description: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub name: String,
    pub version: String,
    pub source: String,
    pub reason: String,
    pub filename: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: Uuid,
    pub draft_id: Option<Uuid>,
    pub scope: Scope,
    pub mod_name: String,
    pub pack_name: String,
    pub explanation: String,
    pub items: Vec<Item>,
    pub changes: Vec<String>,
    pub choices: Vec<String>,
    pub fingerprint: String,
}
struct Pending {
    root: std::path::PathBuf,
    request: Request,
    sender: oneshot::Sender<Decision>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub decision: String,
    pub fingerprint: String,
    pub choice: Option<String>,
    pub file: Option<std::path::PathBuf>,
}
static PENDING: OnceLock<Mutex<HashMap<Uuid, Pending>>> = OnceLock::new();
fn pending() -> &'static Mutex<HashMap<Uuid, Pending>> {
    PENDING.get_or_init(Default::default)
}
fn changed() {
    crate::studio::emit_dependencies();
}
struct Guard(Uuid);
impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut p) = pending().lock() {
            p.remove(&self.0);
        }
        changed();
    }
}

#[allow(clippy::too_many_arguments)] // Assemble an immutable, explicitly scoped UI plan.
pub fn new_request(
    scope: Scope,
    draft_id: Option<Uuid>,
    mod_name: &str,
    pack_name: &str,
    explanation: &str,
    items: Vec<Item>,
    changes: Vec<String>,
    choices: Vec<String>,
) -> Request {
    let fingerprint = bezalel_pack_sync::sha256_hex(
        &serde_json::to_vec(&(
            &scope,
            mod_name,
            pack_name,
            explanation,
            &items,
            &changes,
            &choices,
        ))
        .unwrap(),
    );
    Request {
        id: Uuid::new_v4(),
        draft_id,
        scope,
        mod_name: mod_name.into(),
        pack_name: pack_name.into(),
        explanation: explanation.into(),
        items,
        changes,
        choices,
        fingerprint,
    }
}
pub async fn ask(state: &State, request: Request) -> Result<Decision> {
    // Never persist permission for identity selection or replacements.
    if request.choices.is_empty()
        && request.changes.is_empty()
        && state
            .data()?
            .dependency_grants
            .iter()
            .any(|g| g.scope == request.scope)
    {
        return Ok(Decision {
            decision: "allow".into(),
            fingerprint: request.fingerprint,
            choice: None,
            file: None,
        });
    }
    let id = request.id;
    let (sender, receiver) = oneshot::channel();
    pending().lock().map_err(err)?.insert(
        id,
        Pending {
            root: state.root.clone(),
            request,
            sender,
        },
    );
    let _guard = Guard(id);
    changed();
    let decision = tokio::time::timeout(Duration::from_secs(1800), receiver)
        .await
        .map_err(|_| "Dependency request expired. Retry the operation.")?
        .map_err(|_| "Dependency request canceled.")?;
    if decision.decision == "deny" {
        return Err("Dependency request denied. No mod files were changed.".into());
    }
    Ok(decision)
}
#[tauri::command]
pub fn dependency_requests(state: tauri::State<'_, State>) -> Result<Vec<Request>> {
    list(&state)
}
pub fn list(state: &State) -> Result<Vec<Request>> {
    Ok(pending()
        .lock()
        .map_err(err)?
        .values()
        .filter(|p| p.root == state.root)
        .map(|p| p.request.clone())
        .collect())
}
#[tauri::command]
pub fn dependency_decide(
    state: tauri::State<'_, State>,
    id: Uuid,
    decision: Decision,
) -> Result<()> {
    decide(&state, id, decision)
}
pub fn decide(state: &State, id: Uuid, decision: Decision) -> Result<()> {
    if !["deny", "allow", "always"].contains(&decision.decision.as_str()) {
        return Err("Invalid dependency decision.".into());
    }
    let mut pending = pending().lock().map_err(err)?;
    let p = pending
        .get(&id)
        .filter(|p| p.root == state.root)
        .ok_or("This dependency request is stale or already answered.")?;
    if p.sender.is_closed() || p.request.fingerprint != decision.fingerprint {
        return Err("This dependency plan is no longer active. Refresh it.".into());
    }
    if decision.decision != "deny"
        && !p.request.choices.is_empty()
        && !decision
            .choice
            .as_ref()
            .is_some_and(|c| p.request.choices.contains(c))
    {
        return Err("Choose a verified project or provide a local JAR.".into());
    }
    let mut created_grant = None;
    if decision.decision == "always" {
        if !p.request.choices.is_empty() || !p.request.changes.is_empty() {
            return Err("Replacements and identity choices require one-time approval.".into());
        }
        let scope = p.request.scope.clone();
        let description=format!("{} runtime mod dependencies in {} ({}) using {}. No replacements or changes outside this pack. Revoke here at any time.",scope.action,p.request.pack_name,scope.target_id,scope.sources.join(", "));
        state.update(|data| {
            if !data.dependency_grants.iter().any(|g| g.scope == scope) {
                let grant_id = Uuid::new_v4();
                created_grant = Some(grant_id);
                data.dependency_grants.push(Grant {
                    id: grant_id,
                    scope,
                    description,
                });
            }
            Ok(())
        })?;
    }
    let delivered = pending.remove(&id).unwrap().sender.send(decision);
    drop(pending);
    if delivered.is_err() {
        if let Some(grant_id) = created_grant {
            state.update(|d| {
                d.dependency_grants.retain(|g| g.id != grant_id);
                Ok(())
            })?;
        }
        changed();
        return Err("Dependency request ended; no permission was saved.".into());
    }
    changed();
    Ok(())
}
#[tauri::command]
pub fn dependency_grants(state: tauri::State<'_, State>) -> Result<Vec<Grant>> {
    Ok(state.data()?.dependency_grants)
}
#[tauri::command]
pub fn dependency_revoke(state: tauri::State<'_, State>, id: Uuid) -> Result<()> {
    state.update(|d| {
        d.dependency_grants.retain(|g| g.id != id);
        Ok(())
    })
}
#[tauri::command]
pub fn dependency_cancel(state: tauri::State<'_, State>, id: Uuid) -> Result<()> {
    let mut p = pending().lock().map_err(err)?;
    if p.get(&id).is_none_or(|p| p.root != state.root) {
        return Err("Request already ended.".into());
    }
    p.remove(&id);
    drop(p);
    changed();
    Ok(())
}
pub fn cancel_draft(id: Uuid) {
    if let Ok(mut p) = pending().lock() {
        p.retain(|_, p| p.request.draft_id != Some(id));
    }
    changed();
}
