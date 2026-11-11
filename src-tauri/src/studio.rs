use base64::{engine::general_purpose::URL_SAFE_NO_PAD as BASE64, Engine as _};
use reqwest::{Client, Url};
use ring::{digest, rand::SecureRandom, signature};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    net::TcpListener,
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{oneshot, watch},
    time::timeout,
};

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
static REQUESTS: OnceLock<Mutex<HashMap<String, oneshot::Sender<Value>>>> = OnceLock::new();
pub fn set_app(app: tauri::AppHandle) {
    let _ = APP.set(app);
}
pub fn respond(id: &str, result: Value) -> Result<(), String> {
    REQUESTS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Request lock failed")?
        .remove(id)
        .ok_or("This Codex request is no longer active")?
        .send(result)
        .map_err(|_| "Codex request ended".into())
}
struct PendingRequest(String);
impl Drop for PendingRequest {
    fn drop(&mut self) {
        if let Ok(mut requests) = REQUESTS.get_or_init(Default::default).lock() {
            requests.remove(&self.0);
        }
    }
}
async fn interactive_request(method: &str, params: &Value) -> Result<Value, String> {
    use tauri::Emitter;
    let key = uuid::Uuid::new_v4().to_string();
    let (sender, receiver) = oneshot::channel();
    REQUESTS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "Request lock failed")?
        .insert(key.clone(), sender);
    let _guard = PendingRequest(key.clone());
    APP.get()
        .ok_or("Codex interaction UI is unavailable")?
        .emit(
            "codex-request",
            json!({"requestId": key, "method": method, "params": params}),
        )
        .map_err(message)?;
    receiver
        .await
        .map_err(|_| "Codex request was dismissed".into())
}

const CODEX_VERSION: &str = "0.167.0";
const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const JWKS: &str = "https://auth.openai.com/.well-known/jwks.json";
const DISCOVERY: &str = "https://auth.openai.com/.well-known/openid-configuration";
const RESOURCE: &str = "https://api.openai.com/v1";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const MAX_SOURCE: usize = 8 * 1024 * 1024;
const MAX_RPC_LINE: usize = 1024 * 1024;
const MAX_PROMPT: usize = 49_000;
const INSTRUCTIONS: &str = "You are Codex inside Bezalel, a Minecraft Java mod development app. Work in the draft workspace on a complete mod project. You may read and edit files, run commands, research dependencies, use tools and build or test the project. Preserve the mod ID for existing mods. Loader guidance supplied by Bezalel describes the target and is context, not a restriction on your tools or file types. Explain the result and report actual build/test outcomes. The app can also run its optional isolated Java builder against src/main/java and src/main/resources. Keep personal worlds intact.";
static AUTH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
type ActiveTurns = Mutex<HashMap<String, watch::Sender<bool>>>;
static ACTIVE: OnceLock<ActiveTurns> = OnceLock::new();

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StudioRequest {
    pub draft_id: String,
    pub model_family: String,
    pub reasoning_effort: String,
    pub thread_id: Option<String>,
    pub loader: String,
    pub minecraft_version: String,
    pub mode: String,
    pub prompt: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StudioReply {
    pub thread_id: String,
    pub message: String,
    pub question: Option<StudioQuestion>,
    pub brief: Option<String>,
    pub changes: Vec<StudioChange>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudioQuestion {
    pub text: String,
    pub choices: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudioChange {
    pub path: String,
    pub content: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SavedAccount {
    client_id: String,
    subject: String,
    #[serde(default)]
    tokens: Option<Tokens>,
}

#[derive(Serialize, Deserialize)]
struct Tokens {
    access: String,
    refresh: String,
    id: String,
    scope: String,
    expires_at: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    scope: Option<String>,
    expires_in: u64,
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
    exp: u64,
    nonce: String,
    sub: String,
    azp: Option<String>,
}

#[derive(Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

fn private_dir(root: &Path) -> PathBuf {
    root.join("studio-private")
}

fn account_path(root: &Path) -> PathBuf {
    private_dir(root).join("account.dpapi")
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random() -> Result<String, String> {
    let mut bytes = [0; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "Windows could not generate secure randomness".to_string())?;
    Ok(BASE64.encode(bytes))
}

fn http() -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "Could not initialize the ChatGPT HTTPS client".into())
}

fn read_account(root: &Path) -> Result<Option<SavedAccount>, String> {
    let bytes = match fs::read(account_path(root)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Could not read the local ChatGPT connection".into()),
    };
    let clear = platform::protect(&bytes, false)?;
    serde_json::from_slice(&clear)
        .map(Some)
        .map_err(|_| "The saved ChatGPT connection is invalid".into())
}

fn save_account(root: &Path, account: &SavedAccount) -> Result<(), String> {
    let encoded = serde_json::to_vec(account).map_err(message)?;
    let encrypted = platform::protect(&encoded, true)?;
    write_atomic(&account_path(root), &encrypted)
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = path.parent().ok_or("Missing private data directory")?;
    fs::create_dir_all(directory).map_err(message)?;
    let temporary = directory.join(format!(".write-{}", random()?));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(message)?;
        file.write_all(bytes).map_err(message)?;
        file.sync_all().map_err(message)?;
        drop(file);
        platform::replace(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn has_plan(scopes: &str) -> bool {
    let mut required = [false; 3];
    for scope in scopes.split_ascii_whitespace() {
        match scope {
            PLAN_SCOPE => required[0] = true,
            "resource.invoke" => required[1] = true,
            "offline_access" => required[2] = true,
            _ => {}
        }
    }
    required.into_iter().all(|granted| granted)
}

pub fn connected(root: &Path) -> bool {
    read_account(root)
        .ok()
        .flatten()
        .and_then(|account| account.tokens)
        .is_some_and(|tokens| has_plan(&tokens.scope))
}

pub async fn connect(root: &Path) -> Result<(), String> {
    let _session = AUTH_LOCK.lock().await;
    let prior = read_account(root)?;
    let host_file = private_dir(root).join("host-id");
    let host_id = match fs::read_to_string(&host_file) {
        Ok(host) => host,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let host = format!("urn:uuid:{}", uuid::Uuid::new_v4());
            write_atomic(&host_file, host.as_bytes())?;
            host
        }
        Err(error) => return Err(message(error)),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(message)?;
    let redirect = format!(
        "http://127.0.0.1:{}/auth/callback",
        listener.local_addr().map_err(message)?.port()
    );
    let (state, nonce, verifier) = (random()?, random()?, random()?);
    let challenge = BASE64.encode(digest::digest(&digest::SHA256, verifier.as_bytes()));
    let mut authorization = Url::parse(AUTHORIZE).map_err(message)?;
    {
        let mut params = authorization.query_pairs_mut();
        params.extend_pairs([
            (
                "client_id",
                prior
                    .as_ref()
                    .map_or("dynamic_agent_client", |account| &account.client_id),
            ),
            ("ext_agent_host_id", &host_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("scope", SCOPES),
            ("resource", RESOURCE),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge_method", "S256"),
            ("code_challenge", &challenge),
        ]);
        if let Some(account) = &prior {
            if let Some(tokens) = &account.tokens {
                params.append_pair("id_token_hint", &tokens.id);
            }
        } else {
            params.append_pair("agent_name_hint", "Bezalel");
        }
    }
    platform::open(authorization.as_str())?;
    let (code, issued_id) = timeout(Duration::from_secs(180), oauth_callback(listener, &state))
        .await
        .map_err(|_| "ChatGPT sign-in timed out. Try connecting again.")??;
    let client_id = match (&prior, issued_id) {
        (Some(account), None) => account.client_id.clone(),
        (Some(account), Some(id)) if id == account.client_id => id,
        (None, Some(id)) if id.starts_with("oaiapp_") && id.len() <= 256 => id,
        _ => return Err("ChatGPT returned an unexpected client registration".into()),
    };
    let client = http()?;
    let response = token_request(
        &client,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    let id = response
        .id_token
        .ok_or("ChatGPT did not return an ID token")?;
    let subject = validate_id_token(&client, &id, &client_id, &nonce).await?;
    if prior
        .as_ref()
        .is_some_and(|account| account.subject != subject)
    {
        return Err("Sign in with the ChatGPT account previously connected to Bezalel".into());
    }
    let scope = response
        .scope
        .filter(|scope| has_plan(scope))
        .ok_or("Allow ChatGPT plan usage when connecting Bezalel")?;
    let refresh = response
        .refresh_token
        .ok_or("ChatGPT did not return a renewable session")?;
    save_account(
        root,
        &SavedAccount {
            client_id,
            subject,
            tokens: Some(Tokens {
                access: response.access_token,
                refresh,
                id,
                scope,
                expires_at: now().saturating_add(response.expires_in),
            }),
        },
    )
}

pub(crate) async fn oauth_callback(
    listener: TcpListener,
    state: &str,
) -> Result<(String, Option<String>), String> {
    oauth_callback_at(listener, state, "/auth/callback").await
}

pub(crate) async fn oauth_callback_at(
    listener: TcpListener,
    state: &str,
    path: &str,
) -> Result<(String, Option<String>), String> {
    loop {
        let (mut browser, _) = listener.accept().await.map_err(message)?;
        let mut request = Vec::new();
        let read = timeout(Duration::from_secs(2), async {
            let mut chunk = [0; 512];
            while request.len() < 8192 && !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = browser.read(&mut chunk).await?;
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
            }
            Ok::<_, io::Error>(())
        })
        .await;
        if !matches!(read, Ok(Ok(()))) {
            continue;
        }
        let Some(result) = parse_callback(&request, state, path) else {
            let _ = browser
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            continue;
        };
        let (status, body) = if result.is_ok() {
            (
                "200 OK",
                "Bezalel received your sign-in. Return to the app to finish.",
            )
        } else {
            (
                "400 Bad Request",
                "Sign-in was canceled or denied. Return to Bezalel.",
            )
        };
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = browser.write_all(response.as_bytes()).await;
        return result;
    }
}

type Callback = Result<(String, Option<String>), String>;

fn parse_callback(request: &[u8], state: &str, path: &str) -> Option<Callback> {
    let line = std::str::from_utf8(request).ok()?.lines().next()?;
    let mut parts = line.split_ascii_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let target = parts.next()?;
    if !target.strip_prefix(path)?.starts_with('?') || !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let url = Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    let mut values = HashMap::new();
    for (key, value) in url.query_pairs() {
        if values
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return None;
        }
    }
    if values.get("state").map(String::as_str) != Some(state) {
        return None;
    }
    if values.contains_key("error") {
        return Some(Err("Sign-in was canceled or denied".into()));
    }
    let code = values.remove("code").filter(|code| !code.is_empty())?;
    Some(Ok((code, values.remove("client_id"))))
}

async fn validate_id_token(
    client: &Client,
    id: &str,
    client_id: &str,
    nonce: &str,
) -> Result<String, String> {
    const INVALID: &str = "Could not verify the ChatGPT ID token";
    let mut parts = id.split('.');
    let (header, payload, signature) = (parts.next(), parts.next(), parts.next());
    let (Some(header), Some(payload), Some(signature)) = (header, payload, signature) else {
        return Err(INVALID.into());
    };
    if parts.next().is_some() || id.len() > 16_384 {
        return Err(INVALID.into());
    }
    let header_fields: JwtHeader =
        serde_json::from_slice(&BASE64.decode(header).map_err(|_| INVALID)?)
            .map_err(|_| INVALID)?;
    if header_fields.alg != "RS256" {
        return Err("ChatGPT returned an unsupported ID-token signing algorithm".into());
    }
    let jwks: JwkSet = client
        .get(JWKS)
        .send()
        .await
        .map_err(|_| "Could not retrieve ChatGPT signing keys")?
        .error_for_status()
        .map_err(|_| "Could not retrieve ChatGPT signing keys")?
        .json()
        .await
        .map_err(|_| INVALID)?;
    let key = jwks
        .keys
        .into_iter()
        .find(|key| {
            key.kid == header_fields.kid
                && key.kty == "RSA"
                && key.alg.as_deref().is_none_or(|alg| alg == "RS256")
        })
        .ok_or(INVALID)?;
    let n = BASE64.decode(key.n.ok_or(INVALID)?).map_err(|_| INVALID)?;
    let e = BASE64.decode(key.e.ok_or(INVALID)?).map_err(|_| INVALID)?;
    let raw_signature = BASE64.decode(signature).map_err(|_| INVALID)?;
    signature::RsaPublicKeyComponents { n: &n, e: &e }
        .verify(
            &signature::RSA_PKCS1_2048_8192_SHA256,
            format!("{header}.{payload}").as_bytes(),
            &raw_signature,
        )
        .map_err(|_| INVALID)?;
    let claims: Claims = serde_json::from_slice(&BASE64.decode(payload).map_err(|_| INVALID)?)
        .map_err(|_| INVALID)?;
    let audience_matches = match claims.aud {
        Audience::Single(audience) => audience == client_id,
        Audience::Multiple(audience) => {
            audience.iter().any(|entry| entry == client_id)
                && (audience.len() == 1 || claims.azp.as_deref() == Some(client_id))
        }
    };
    if claims.iss != ISSUER
        || !audience_matches
        || claims.exp <= now()
        || claims.nonce != nonce
        || claims.sub.is_empty()
        || claims
            .azp
            .as_deref()
            .is_some_and(|party| party != client_id)
    {
        return Err(INVALID.into());
    }
    Ok(claims.sub)
}

async fn token_request(client: &Client, fields: &[(&str, &str)]) -> Result<TokenResponse, String> {
    let response = post_form(client, TOKEN, fields).await?;
    if !response.status().is_success() {
        return Err("ChatGPT did not authorize this session. Reconnect from Settings.".into());
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|_| "ChatGPT returned an invalid token response")?;
    if tokens.expires_in == 0 || tokens.access_token.is_empty() {
        return Err("ChatGPT returned an expired session".into());
    }
    Ok(tokens)
}

async fn post_form(
    client: &Client,
    endpoint: &str,
    fields: &[(&str, &str)],
) -> Result<reqwest::Response, String> {
    let mut encoded = Url::parse(ISSUER).map_err(message)?;
    encoded
        .query_pairs_mut()
        .extend_pairs(fields.iter().copied());
    client
        .post(endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(encoded.query().unwrap_or_default().to_owned())
        .send()
        .await
        .map_err(|_| "Could not reach ChatGPT. Check your connection and try again.".into())
}

async fn active_token(root: &Path) -> Result<(String, String), String> {
    let _session = AUTH_LOCK.lock().await;
    let mut account = read_account(root)?.ok_or("Connect ChatGPT in Settings to create a mod")?;
    let tokens = account
        .tokens
        .as_mut()
        .ok_or("Connect ChatGPT in Settings to create a mod")?;
    if tokens.expires_at <= now().saturating_add(120) {
        let response = token_request(
            &http()?,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", &account.client_id),
                ("refresh_token", &tokens.refresh),
                ("resource", RESOURCE),
            ],
        )
        .await?;
        if let Some(refresh) = response.refresh_token {
            tokens.refresh = refresh;
        }
        if let Some(scope) = response.scope {
            tokens.scope = scope;
        }
        tokens.access = response.access_token;
        tokens.expires_at = now().saturating_add(response.expires_in);
        save_account(root, &account)?;
    }
    let tokens = account
        .tokens
        .ok_or("Connect ChatGPT in Settings to create a mod")?;
    if !has_plan(&tokens.scope) {
        return Err("Enable ChatGPT plan usage to create mods".into());
    }
    Ok((tokens.access, account.client_id))
}

pub async fn disconnect(root: &Path) -> Result<(), String> {
    let _session = AUTH_LOCK.lock().await;
    if let Ok(active) = active_turns().lock() {
        for sender in active.values() {
            let _ = sender.send(true);
        }
    }
    let Some(mut account) = read_account(root)? else {
        return Ok(());
    };
    let Some(tokens) = account.tokens.take() else {
        return Ok(());
    };
    save_account(root, &account)?;
    if revoke(&account.client_id, &tokens.refresh).await.is_err() {
        return Err("ChatGPT was disconnected on this PC. Remote revocation could not be confirmed. You can revoke Bezalel in ChatGPT Settings.".into());
    }
    Ok(())
}

async fn revoke(client_id: &str, refresh: &str) -> Result<(), String> {
    let client = http()?;
    let discovery: Value = client
        .get(DISCOVERY)
        .send()
        .await
        .map_err(message)?
        .error_for_status()
        .map_err(message)?
        .json()
        .await
        .map_err(message)?;
    let endpoint = Url::parse(
        discovery["revocation_endpoint"]
            .as_str()
            .ok_or("Missing revocation endpoint")?,
    )
    .map_err(message)?;
    if endpoint.origin().ascii_serialization() != ISSUER
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err("Unexpected ChatGPT revocation endpoint".into());
    }
    let response = post_form(
        &client,
        endpoint.as_str(),
        &[
            ("token", refresh),
            ("token_type_hint", "refresh_token"),
            ("client_id", client_id),
        ],
    )
    .await?;
    response.error_for_status().map(|_| ()).map_err(message)
}

fn active_turns() -> &'static ActiveTurns {
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

struct TurnGuard(String);

impl Drop for TurnGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = active_turns().lock() {
            active.remove(&self.0);
        }
    }
}

pub fn cancel(draft_id: &str) -> bool {
    active_turns().lock().ok().is_some_and(|active| {
        active
            .get(draft_id)
            .is_some_and(|sender| sender.send(true).is_ok())
    })
}

pub async fn run(
    root: &Path,
    req: StudioRequest,
    emit: impl Fn(String) + Send + Sync,
) -> Result<StudioReply, String> {
    if req.draft_id.is_empty()
        || req.draft_id.len() > 64
        || !req
            .draft_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err("Invalid mod draft identifier".into());
    }
    if req.minecraft_version != "1.21.1" {
        return Err("This Minecraft version has no Bezalel Studio skill".into());
    }
    if req.prompt.trim().is_empty() || req.prompt.len() > MAX_PROMPT {
        return Err("This message exceeds the Studio input limit".into());
    }
    let mode = Mode::parse(&req.mode)?;
    let skill = match req.loader.to_ascii_lowercase().as_str() {
        "fabric" => include_str!("../resources/skills/fabric/SKILL.md"),
        "forge" => include_str!("../resources/skills/forge/SKILL.md"),
        "neoforge" => include_str!("../resources/skills/neoforge/SKILL.md"),
        _ => return Err("This loader has no Bezalel Studio skill".into()),
    };
    let (sender, mut cancellation) = watch::channel(false);
    {
        let mut active = active_turns()
            .lock()
            .map_err(|_| "Could not start a Studio turn")?;
        if active.contains_key(&req.draft_id) {
            return Err("This draft already has a turn running".into());
        }
        active.insert(req.draft_id.clone(), sender);
    }
    let _guard = TurnGuard(req.draft_id.clone());
    let prepared = async {
        let (access, client_id) = active_token(root).await?;
        let options = crate::agent_models::catalog(&load_models(&access).await?)?;
        let (model, effort) =
            crate::agent_models::select(&options, &req.model_family, &req.reasoning_effort)?;
        let client_key = BASE64.encode(digest::digest(&digest::SHA256, client_id.as_bytes()));
        let home = private_dir(root).join("runtime").join(client_key);
        let cwd = root.join("drafts").join(&req.draft_id);
        fs::create_dir_all(&cwd).map_err(message)?;

        let mut rpc = Rpc::launch(&home, &cwd, &access).await?;
        let thread_id = rpc
            .open_thread(&cwd, &model, req.thread_id.as_deref())
            .await?;
        if let Some(app) = APP.get() {
            use tauri::Manager;
            if let Some(state) = app.try_state::<crate::state::State>() {
                let _ = state.update(|data| {
                    crate::state::update_draft(
                        data,
                        uuid::Uuid::parse_str(&req.draft_id).map_err(message)?,
                        |draft| draft.thread_id = Some(thread_id.clone()),
                    )
                });
            }
        }
        let params = json!({
            "threadId": thread_id,
            "model": model,
            "effort": effort,
            "approvalPolicy": "never",
            "sandboxPolicy": { "type": "dangerFullAccess" },
            "input": [
                {
                    "type": "text",
                    "text": format!("{}\nTarget: {} {}.\n\nLoader guidance:\n{skill}\n\nUser message:\n{}", mode.prompt(), req.loader, req.minecraft_version, req.prompt)
                }
            ],
        });

        let started = rpc.request("turn/start", params).await?;
        let turn_id = started["turn"]["id"]
            .as_str()
            .ok_or("Codex did not start the turn")?
            .to_owned();
        Ok::<_, String>((rpc, thread_id, turn_id))
    };
    let (mut rpc, thread_id, turn_id) = tokio::select! {
        biased;
        _ = cancellation.changed() => return Err("Turn canceled".into()),
        result = timeout(Duration::from_secs(45), prepared) => {
            result.map_err(|_| "Codex did not start in time")??
        }
    };
    let (result, interrupt) = tokio::select! {
        biased;
        _ = cancellation.changed() => (Err("Turn canceled".into()), true),
        result = timeout(Duration::from_secs(7200), rpc.read_turn(&thread_id, &turn_id, mode, &emit)) => {
            match result {
                Ok(reply) => (reply, false),
                Err(_) => (Err("Codex timed out".into()), true),
            }
        }
    };
    if interrupt {
        let _ = timeout(
            Duration::from_secs(2),
            rpc.request(
                "turn/interrupt",
                json!({ "threadId": thread_id, "turnId": turn_id }),
            ),
        )
        .await;
    }
    rpc.close().await;
    result.map(|text| StudioReply {
        thread_id,
        message: text,
        question: None,
        brief: None,
        changes: source_changes(root, &req.draft_id).unwrap_or_default(),
    })
}

pub async fn models(root: &Path) -> Result<Vec<crate::agent_models::AgentModel>, String> {
    let (access, _) = active_token(root).await?;
    crate::agent_models::catalog(&load_models(&access).await?)
}

async fn load_models(access: &str) -> Result<Value, String> {
    let response = http()?
        .get(format!("{RESOURCE}/models"))
        .bearer_auth(access)
        .send()
        .await
        .map_err(|_| "Could not load models for your ChatGPT plan")?;
    match response.status().as_u16() {
        200..=299 => {}
        401 => return Err("ChatGPT sign-in expired. Reconnect in Settings.".into()),
        403 => return Err("This ChatGPT account does not have plan access for Bezalel.".into()),
        429 => return Err("ChatGPT is limiting requests. Check plan usage and try again.".into()),
        _ => return Err("Could not load the ChatGPT plan catalog. Try again shortly.".into()),
    }
    let data: Value = response
        .json()
        .await
        .map_err(|_| "ChatGPT returned an invalid model list")?;
    Ok(data)
}

#[derive(Clone, Copy)]
enum Mode {
    Direct,
    Guided,
    Build,
}

impl Mode {
    fn parse(mode: &str) -> Result<Self, String> {
        match mode {
            "direct" => Ok(Self::Direct),
            "guided" => Ok(Self::Guided),
            "build" => Ok(Self::Build),
            _ => Err("Unknown Studio mode".into()),
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::Direct => "Carry out the user request in the mod workspace, including writing and testing complete mods when asked.",
            Self::Guided => "Ask one focused question at a time about an unresolved feature. Include 2–3 short options if meaningful. A free-text answer is always supported. Return either one question or an editable brief after the major requirements are clear. Do not propose files until the player clicks Build.",
            Self::Build => "Implement the complete mod in this workspace. Write the project files directly, including Java source, loader metadata, resources and build configuration. Run appropriate checks and summarize the outcome.",
        }
    }
}

fn source_path_allowed(path: &str) -> bool {
    if path.len() > 220
        || path.split('/').any(|part| {
            let base = part
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let device = ["con", "prn", "aux", "nul"].contains(&base.as_str())
                || ((base.starts_with("com") || base.starts_with("lpt"))
                    && base.len() == 4
                    && base.as_bytes()[3].is_ascii_digit());
            part.is_empty()
                || part.starts_with('.')
                || part.ends_with('.')
                || device
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        })
    {
        return false;
    }
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    (path.starts_with("src/main/java/") && extension == Some("java"))
        || (path.starts_with("src/main/resources/")
            && matches!(
                extension,
                Some("json" | "mcmeta" | "toml" | "properties" | "txt" | "lang")
            ))
}

#[cfg(test)]
fn validate_changes(changes: &[StudioChange]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    if changes.len() > 64
        || changes
            .iter()
            .map(|change| change.content.as_ref().map_or(0, String::len))
            .sum::<usize>()
            > MAX_SOURCE
    {
        return Err("Codex proposed too many files or too much source for one build. Split the request into smaller edits.".into());
    }
    for change in changes {
        if !source_path_allowed(&change.path) || !seen.insert(change.path.to_ascii_lowercase()) {
            return Err("Codex proposed duplicate or unsupported draft file paths".into());
        }
    }
    Ok(())
}

pub fn source_changes(root: &Path, id: &str) -> Result<Vec<StudioChange>, String> {
    let sources: Vec<(String, String)> =
        serde_json::from_str(&draft_sources(root, id)?).map_err(message)?;
    Ok(sources
        .into_iter()
        .map(|(path, content)| StudioChange {
            path,
            content: Some(content),
        })
        .collect())
}

fn draft_sources(root: &Path, draft_id: &str) -> Result<String, String> {
    let drafts = root.join("drafts");
    let draft = drafts.join(draft_id);
    let src = draft.join("src");
    for directory in [&drafts, &draft, &src] {
        match fs::symlink_metadata(directory) {
            Ok(meta) if meta.is_dir() && !platform::linked(&meta) => {}
            Ok(_) => return Err("Draft source must be stored in regular directories".into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok("[]".into()),
            Err(error) => return Err(message(error)),
        }
    }
    let mut files = Vec::new();
    let mut pending = vec![src];
    let mut entries = 0;
    let mut total = 0;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).map_err(message)? {
            entries += 1;
            if entries > 16384 {
                return Err("Draft has too many source entries".into());
            }
            let path = entry.map_err(message)?.path();
            let meta = fs::symlink_metadata(&path).map_err(message)?;
            if platform::linked(&meta) {
                return Err("Draft source contains a link".into());
            }
            if meta.is_dir() {
                pending.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(&draft)
                .map_err(message)?
                .to_string_lossy()
                .replace('\\', "/");
            if !meta.is_file() || !source_path_allowed(&relative) {
                continue;
            }
            if meta.len() > MAX_SOURCE as u64 || total + meta.len() as usize > MAX_SOURCE {
                return Err("Draft sources exceed the Studio context limit".into());
            }
            let Ok(content) = fs::read_to_string(path) else {
                continue;
            };
            total += content.len();
            if total > MAX_SOURCE {
                return Err("Draft sources exceed the Studio context limit".into());
            }
            files.push((relative, content));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    serde_json::to_string(&files).map_err(message)
}

struct Rpc {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    queued: VecDeque<Value>,
}

impl Rpc {
    async fn launch(home: &Path, cwd: &Path, access: &str) -> Result<Self, String> {
        let executable = codex_binary()?;
        let temp = home.join("temp");
        fs::create_dir_all(&temp).map_err(message)?;
        let command = || {
            let mut command = Command::new(&executable);
            command
                .current_dir(cwd)
                .env("CODEX_HOME", home)
                .env("TEMP", &temp)
                .env("TMP", &temp)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            for key in ["SystemRoot", "SystemDrive", "WINDIR", "COMSPEC"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            #[cfg(windows)]
            command.creation_flags(0x0800_0000);
            command
        };
        let version = timeout(Duration::from_secs(8), command().arg("--version").output())
            .await
            .map_err(|_| "Could not check the Codex executable version")?
            .map_err(|_| "Could not run the Codex executable")?;
        if !version.status.success()
            || String::from_utf8_lossy(&version.stdout).trim()
                != format!("codex-cli {CODEX_VERSION}")
        {
            return Err(format!("Bezalel Studio requires Codex CLI {CODEX_VERSION}"));
        }
        let mut child = command()
            .args(["app-server", "--listen", "stdio://"])
            .args([
                "-c",
                "model_provider=\"openai_chatgpt_plan\"",
                "-c",
                "model_providers.openai_chatgpt_plan.name=\"ChatGPT plan\"",
                "-c",
                "model_providers.openai_chatgpt_plan.base_url=\"https://api.openai.com/v1\"",
                "-c",
                "model_providers.openai_chatgpt_plan.env_key=\"ACCESS_TOKEN\"",
                "-c",
                "model_providers.openai_chatgpt_plan.wire_api=\"responses\"",
                "-c",
                "model_providers.openai_chatgpt_plan.requires_openai_auth=false",
                "-c",
                "model_providers.openai_chatgpt_plan.supports_websockets=false",
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "sandbox_mode=\"danger-full-access\"",
            ])
            .env("ACCESS_TOKEN", access)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|_| "Could not start Codex App Server")?;
        let input = child.stdin.take().ok_or("Codex stdin is unavailable")?;
        let output = child.stdout.take().ok_or("Codex stdout is unavailable")?;
        let mut rpc = Self {
            child,
            input,
            output: BufReader::new(output).lines(),
            next_id: 0,
            queued: VecDeque::new(),
        };
        rpc.request("initialize", json!({
            "clientInfo": { "name": "bezalel", "title": "Bezalel", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "experimentalApi": true }
        })).await?;
        rpc.send(json!({ "method": "initialized" })).await?;
        Ok(rpc)
    }

    async fn close(self) {
        let Self {
            mut child, input, ..
        } = self;
        drop(input);
        let _ = timeout(Duration::from_secs(2), child.wait()).await;
    }

    async fn open_thread(
        &mut self,
        cwd: &Path,
        model: &str,
        resume: Option<&str>,
    ) -> Result<String, String> {
        let mut params = json!({
            "cwd": cwd,
            "model": model,
            "developerInstructions": INSTRUCTIONS,
            "approvalPolicy": "never",
            "sandbox": "danger-full-access",
        });
        let method = if let Some(thread) = resume {
            params["threadId"] = json!(thread);
            params["excludeTurns"] = json!(true);
            "thread/resume"
        } else {
            "thread/start"
        };
        let result = self.request(method, params).await?;
        result["thread"]["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or("Codex did not open the draft thread".into())
    }

    async fn send(&mut self, value: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&value).map_err(message)?;
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|_| "Codex App Server disconnected".into())
    }

    async fn receive(&mut self) -> Result<Value, String> {
        loop {
            let line = self
                .output
                .next_line()
                .await
                .map_err(|_| "Could not read Codex App Server output")?
                .ok_or("Codex App Server stopped")?;
            if line.len() > MAX_RPC_LINE {
                return Err("Codex exceeded the Studio message limit".into());
            }
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line)
                .map_err(|_| "Codex returned an invalid protocol message")?;
            if let (Some(method), Some(id)) = (value["method"].as_str(), value.get("id")) {
                let result = interactive_request(method, &value["params"]).await?;
                let response = json!({"id": id, "result": result});
                self.send(response).await?;
                continue;
            }
            if value.get("method").is_some() {
                use tauri::Emitter;
                if let Some(app) = APP.get() {
                    let _ = app.emit("codex-event", &value);
                }
            }
            return Ok(value);
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "method": method, "id": id, "params": params }))
            .await?;
        loop {
            let value = self.receive().await?;
            if value["id"].as_u64() == Some(id) {
                return value.get("result").cloned().ok_or_else(|| {
                    if method == "thread/resume" {
                        "Could not reopen this Codex thread. Reconnect its ChatGPT account or start a new draft.".into()
                    } else {
                        format!("Codex could not complete {method}")
                    }
                });
            }
            if value["method"].as_str().is_some_and(|event| {
                matches!(
                    event,
                    "turn/completed" | "item/completed" | "item/agentMessage/delta"
                )
            }) {
                if self.queued.len() >= 256 {
                    return Err("Codex exceeded the Studio notification limit".into());
                }
                self.queued.push_back(value);
            }
        }
    }

    async fn read_turn(
        &mut self,
        thread: &str,
        turn: &str,
        mode: Mode,
        emit: &impl Fn(String),
    ) -> Result<String, String> {
        let mut final_text = None;
        let mut delta = String::new();
        loop {
            let value = match self.queued.pop_front() {
                Some(value) => value,
                None => self.receive().await?,
            };
            let params = &value["params"];
            if params["threadId"].as_str() != Some(thread) {
                continue;
            }
            match value["method"].as_str() {
                Some("item/agentMessage/delta") if params["turnId"].as_str() == Some(turn) => {
                    if let Some(part) = params["delta"].as_str() {
                        delta.push_str(part);
                        if delta.len() > MAX_RPC_LINE {
                            return Err("Codex exceeded the Studio response limit".into());
                        }
                        if matches!(mode, Mode::Direct | Mode::Guided | Mode::Build) {
                            emit(part.to_owned());
                        }
                    }
                }
                Some("item/completed") if params["turnId"].as_str() == Some(turn) => {
                    let item = &params["item"];
                    if item["type"] == "agentMessage" && item["phase"] != "commentary" {
                        final_text = item["text"].as_str().map(str::to_owned);
                    }
                }
                Some("turn/completed") if params["turn"]["id"].as_str() == Some(turn) => {
                    let completed = &params["turn"];
                    return match completed["status"].as_str() {
                        Some("completed") => {
                            let text = final_text.unwrap_or(delta);
                            if text.trim().is_empty() {
                                Err("Codex returned an empty reply".into())
                            } else {
                                Ok(text)
                            }
                        }
                        Some("interrupted") => Err("Turn canceled".into()),
                        _ => Err(turn_error(&completed["error"])),
                    };
                }
                _ => {}
            }
        }
    }
}

fn turn_error(error: &Value) -> String {
    let detail = error["message"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ["429", "rate limit", "usage limit", "quota"]
        .iter()
        .any(|term| detail.contains(term))
    {
        "ChatGPT plan usage limit reached. Check your usage in ChatGPT Settings.".into()
    } else if ["401", "403", "unauthorized", "forbidden", "expired"]
        .iter()
        .any(|term| detail.contains(term))
    {
        "ChatGPT did not authorize this turn. Reconnect in Settings.".into()
    } else {
        "Codex could not finish this turn. Try again.".into()
    }
}

fn codex_binary() -> Result<PathBuf, String> {
    let filename = if cfg!(windows) { "codex.exe" } else { "codex" };
    if let Some(path) = std::env::var_os("BEZALEL_CODEX_PATH").map(PathBuf::from) {
        return path
            .is_absolute()
            .then(|| path.canonicalize().ok())
            .flatten()
            .filter(|path| path.is_file())
            .ok_or("BEZALEL_CODEX_PATH must point to a Codex executable".into());
    }
    let app_path = std::env::var_os("LOCALAPPDATA").map(|path| {
        PathBuf::from(path)
            .join("Programs/OpenAI/Codex/bin")
            .join(filename)
    });
    let paths = std::env::var_os("PATH").unwrap_or_default();
    app_path
        .into_iter()
        .chain(std::env::split_paths(&paths).map(|dir| dir.join(filename)))
        .filter(|path| path.is_absolute())
        .find_map(|path| path.is_file().then(|| path.canonicalize().ok()).flatten())
        .ok_or_else(|| {
            format!(
                "Install Codex CLI {CODEX_VERSION}, or set BEZALEL_CODEX_PATH to its executable"
            )
        })
}

#[cfg(windows)]
pub(crate) mod platform {
    use std::{
        ffi::{c_void, OsStr},
        fs::Metadata,
        io,
        os::windows::{ffi::OsStrExt, fs::MetadataExt},
        path::Path,
        ptr,
    };

    #[repr(C)]
    struct Blob {
        size: u32,
        data: *mut u8,
    }

    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptProtectData(
            input: *const Blob,
            description: *const u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const Blob,
            description: *mut *mut u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            window: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain([0]).collect()
    }

    pub fn open(url: &str) -> Result<(), String> {
        let operation = wide(OsStr::new("open"));
        let url = wide(OsStr::new(url));
        // ShellExecuteW receives two owned, nul-terminated strings for the duration of the call.
        let result = unsafe {
            ShellExecuteW(
                ptr::null_mut(),
                operation.as_ptr(),
                url.as_ptr(),
                ptr::null(),
                ptr::null(),
                1,
            )
        };
        if result <= 32 {
            Err("Could not open the system browser for sign-in".into())
        } else {
            Ok(())
        }
    }

    pub fn protect(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
        let input = Blob {
            size: u32::try_from(bytes.len()).map_err(|_| "Credential data is too large")?,
            data: bytes.as_ptr().cast_mut(),
        };
        let mut output = Blob {
            size: 0,
            data: ptr::null_mut(),
        };
        // DPAPI borrows the input and returns a LocalFree-owned output. The UI-forbidden flag
        // prevents an unexpected OS prompt when the current Windows profile cannot decrypt it.
        let ok = unsafe {
            if encrypt {
                CryptProtectData(
                    &input,
                    ptr::null(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    1,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    1,
                    &mut output,
                )
            }
        };
        if ok == 0 || output.data.is_null() {
            return Err("Windows could not protect the connection for this user".into());
        }
        let result = unsafe {
            let copied = std::slice::from_raw_parts(output.data, output.size as usize).to_vec();
            LocalFree(output.data.cast());
            copied
        };
        Ok(result)
    }

    fn move_file(from: &Path, to: &Path, flags: u32) -> io::Result<()> {
        let from = wide(from.as_os_str());
        let to = wide(to.as_os_str());
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn replace(from: &Path, to: &Path) -> Result<(), String> {
        // Replace the existing encrypted record and write through on the same volume.
        move_file(from, to, 1 | 8).map_err(|error| error.to_string())
    }

    pub fn persist_new(from: &Path, to: &Path) -> io::Result<()> {
        // Publish the staged file without replacing another mod of the same name.
        move_file(from, to, 8)
    }

    pub fn linked(meta: &Metadata) -> bool {
        meta.file_attributes() & 0x400 != 0
    }
}

#[cfg(not(windows))]
pub(crate) mod platform {
    use std::{
        fs::{self, Metadata},
        io,
        path::Path,
    };

    pub fn open(_: &str) -> Result<(), String> {
        Err("Bezalel sign-in requires Windows".into())
    }
    pub fn protect(_: &[u8], _: bool) -> Result<Vec<u8>, String> {
        Err("Bezalel credentials require Windows DPAPI".into())
    }
    pub fn replace(from: &Path, to: &Path) -> Result<(), String> {
        fs::rename(from, to).map_err(|error| error.to_string())
    }
    pub fn persist_new(from: &Path, to: &Path) -> io::Result<()> {
        fs::hard_link(from, to)
    }
    pub fn linked(meta: &Metadata) -> bool {
        meta.file_type().is_symlink()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_local_callback(request: &[u8], state: &str) -> Option<Callback> {
        parse_callback(request, state, "/auth/callback")
    }

    #[test]
    fn callback_checks_path_state_and_duplicate_fields() {
        assert!(parse_local_callback(
            b"GET /auth/callback?code=x&state=correct HTTP/1.1\r\n",
            "correct"
        )
        .unwrap()
        .is_ok());
        assert!(parse_local_callback(
            b"GET /auth/callback?error=access_denied&state=correct HTTP/1.1\r\n",
            "correct"
        )
        .unwrap()
        .is_err());
        assert!(parse_local_callback(
            b"GET /auth/callback?code=x&state=bad HTTP/1.1\r\n",
            "correct"
        )
        .is_none());
        assert!(parse_local_callback(
            b"GET /auth/callback?code=x&state=correct&state=correct HTTP/1.1\r\n",
            "correct"
        )
        .is_none());
        assert!(
            parse_local_callback(b"GET /other?code=x&state=correct HTTP/1.1\r\n", "correct")
                .is_none()
        );
    }

    #[tokio::test]
    #[ignore = "Reads the live account model catalog"]
    async fn live_model_families() {
        let root = PathBuf::from(std::env::var("BEZALEL_HARNESS_SMOKE_ROOT").unwrap());
        println!(
            "{}",
            serde_json::to_string_pretty(&models(&root).await.unwrap()).unwrap()
        );
    }

    #[tokio::test]
    #[ignore = "Live ChatGPT harness smoke; creates a separate probe workspace"]
    async fn full_harness_executes_commands_and_writes_workspace_files() {
        let root = PathBuf::from(
            std::env::var("BEZALEL_HARNESS_SMOKE_ROOT")
                .expect("Select the signed-in app data root"),
        );
        let id = uuid::Uuid::new_v4().to_string();
        let reply = run(&root, StudioRequest { model_family: "Sol".into(), reasoning_effort: "xhigh".into(), draft_id: id.clone(), thread_id: None,
            loader: "forge".into(), minecraft_version: "1.21.1".into(), mode: "direct".into(),
            prompt: "Use your terminal tool to write a file named harness-probe.txt in the current workspace containing exactly harness-exec-ok. Then use the terminal to read it back. Do not change other files. Report the result.".into() }, |_| {}).await.expect("Full harness turn");
        assert!(!reply.thread_id.is_empty());
        let workspace = root.join("drafts").join(&id);
        assert_eq!(
            fs::read_to_string(workspace.join("harness-probe.txt"))
                .unwrap()
                .trim(),
            "harness-exec-ok"
        );
        run(&root, StudioRequest { model_family: "Luna".into(), reasoning_effort: "medium".into(), draft_id: id.clone(), thread_id: Some(reply.thread_id), loader: "forge".into(), minecraft_version: "1.21.1".into(), mode: "direct".into(), prompt: "Use your terminal tool to read harness-probe.txt, then write resume-ok into a new file resume-probe.txt. Do not change other files.".into() }, |_| {}).await.expect("Resume full harness thread");
        assert_eq!(
            fs::read_to_string(workspace.join("resume-probe.txt"))
                .unwrap()
                .trim(),
            "resume-ok"
        );
        println!(
            "Harness command execution and workspace file write passed in {}",
            workspace.display()
        );
    }

    #[test]
    fn build_paths_exclude_traversal_devices_and_build_files() {
        assert!(source_path_allowed("src/main/java/bezel/Example.java"));
        assert!(source_path_allowed(
            "src/main/resources/META-INF/neoforge.mods.toml"
        ));
        assert!(!source_path_allowed("src/main/java/../Escape.java"));
        assert!(!source_path_allowed("src/main/java/CON.java"));
        assert!(!source_path_allowed("src/main/java/Escape.java:stream"));
        assert!(!source_path_allowed("src/main/resources/build.gradle"));
        assert!(!source_path_allowed("build.gradle"));
        assert!(validate_changes(&[
            StudioChange {
                path: "src/main/java/Example.java".into(),
                content: None
            },
            StudioChange {
                path: "src/main/java/example.java".into(),
                content: None
            },
        ])
        .is_err());
    }
}
