use super::Profile;
use crate::studio::{oauth_callback, platform, write_atomic};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use reqwest::{Client, Response, StatusCode, Url};
use ring::{digest, rand::SecureRandom};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpListener, sync::Mutex, time::timeout};
use uuid::Uuid;

const AUTHORIZE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const TOKEN: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const SCOPES: &str = "XboxLive.signin offline_access";
const XBL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MINECRAFT: &str = "https://api.minecraftservices.com";
static SESSION_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Deserialize, Serialize)]
struct Session {
    client_id: String,
    microsoft_refresh: String,
    game: GameSession,
}

#[derive(Deserialize, Serialize)]
struct GameSession {
    id: String,
    name: String,
    xuid: String,
    access_token: String,
    expires_at: u64,
}

impl Session {
    fn profile(self) -> Profile {
        Profile {
            id: self.game.id,
            name: self.game.name,
            access_token: self.game.access_token,
            xuid: self.game.xuid,
            client_id: self.client_id,
        }
    }
}

#[derive(Deserialize)]
struct MicrosoftToken {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XboxToken {
    token: String,
    #[serde(default)]
    display_claims: XboxClaims,
}

#[derive(Default, Deserialize)]
struct XboxClaims {
    #[serde(default)]
    xui: Vec<XboxUser>,
}

#[derive(Deserialize)]
struct XboxUser {
    #[serde(default)]
    uhs: String,
    #[serde(default, rename = "xid", alias = "xuid")]
    xuid: Option<String>,
}

#[derive(Deserialize)]
struct MinecraftToken {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct Entitlements {
    items: Vec<Entitlement>,
}

#[derive(Deserialize)]
struct Entitlement {
    name: String,
}

#[derive(Deserialize)]
struct GameProfile {
    id: String,
    name: String,
}

pub async fn connect(root: &Path, client_id: &str) -> Result<String, String> {
    let client_id = app_id(client_id)?;
    let _session = SESSION_LOCK.lock().await;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "Could not start the Microsoft sign-in callback")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Could not read the Microsoft sign-in callback address")?
        .port();
    // Register http://localhost/auth/callback as a Mobile and desktop application URI.
    // Microsoft allows native localhost redirect registrations to use an ephemeral port.
    let redirect = format!("http://localhost:{port}/auth/callback");
    let state = random_verifier()?;
    let verifier = random_verifier()?;
    let challenge = URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, verifier.as_bytes()));
    let mut authorize = Url::parse(AUTHORIZE).map_err(|_| "Invalid Microsoft authorization URL")?;
    authorize.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("response_type", "code"),
        ("response_mode", "query"),
        ("redirect_uri", redirect.as_str()),
        ("scope", SCOPES),
        ("state", state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("prompt", "select_account"),
    ]);
    platform::open(authorize.as_str())?;
    let (code, _) = timeout(Duration::from_secs(180), oauth_callback(listener, &state))
        .await
        .map_err(|_| "Microsoft sign-in timed out. Try connecting again.")??;
    let client = http()?;
    let token = microsoft_token(
        &client,
        &[
            ("client_id", &client_id),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("code_verifier", &verifier),
            ("scope", SCOPES),
        ],
    )
    .await?;
    let microsoft_refresh = token
        .refresh_token
        .filter(|token| !token.is_empty())
        .ok_or("Microsoft did not grant a renewable sign-in")?;
    let game = minecraft_session(&client, &token.access_token).await?;
    let name = game.name.clone();
    save_session(
        root,
        &Session {
            client_id,
            microsoft_refresh,
            game,
        },
    )?;
    Ok(name)
}

pub fn connected(root: &Path) -> Option<String> {
    read_session(root)
        .ok()
        .flatten()
        .map(|session| session.game.name)
}

pub fn disconnect(root: &Path) -> Result<(), String> {
    let _session = SESSION_LOCK
        .try_lock()
        .map_err(|_| "Wait for the current Microsoft sign-in to finish")?;
    match fs::remove_file(session_path(root)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Could not remove this PC's saved Microsoft sign-in".into()),
    }
}

pub async fn refresh(root: &Path, client_id: &str) -> Result<Profile, String> {
    let client_id = app_id(client_id)?;
    let _session = SESSION_LOCK.lock().await;
    let mut session =
        read_session(root)?.ok_or("Connect Microsoft / Minecraft in Settings before playing")?;
    if session.client_id != client_id {
        return Err(
            "The Microsoft app ID changed. Reconnect your Minecraft account in Settings.".into(),
        );
    }
    if session.game.expires_at > now().saturating_add(120) {
        return Ok(session.profile());
    }
    let client = http()?;
    let token = microsoft_token(
        &client,
        &[
            ("client_id", &client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", &session.microsoft_refresh),
            ("scope", SCOPES),
        ],
    )
    .await?;
    if let Some(refresh) = token.refresh_token.filter(|token| !token.is_empty()) {
        session.microsoft_refresh = refresh;
        // Save the rotated Microsoft credential before making more requests.
        save_session(root, &session)?;
    }
    let game = minecraft_session(&client, &token.access_token).await?;
    if game.id != session.game.id {
        return Err(
            "Microsoft returned a different Minecraft profile. Sign in again from Settings.".into(),
        );
    }
    session.game = game;
    save_session(root, &session)?;
    Ok(session.profile())
}

fn app_id(input: &str) -> Result<String, String> {
    let id = Uuid::parse_str(input.trim())
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or("Set Bezalel's approved Microsoft application ID before connecting Minecraft")?;
    Ok(id.hyphenated().to_string())
}

fn http() -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Could not initialize Minecraft sign-in".into())
}

fn random_verifier() -> Result<String, String> {
    let mut bytes = [0; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "Could not generate a secure Microsoft sign-in challenge")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn session_path(root: &Path) -> PathBuf {
    root.join("accounts").join("minecraft.dpapi")
}

fn read_session(root: &Path) -> Result<Option<Session>, String> {
    let encrypted = match fs::read(session_path(root)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Could not read this PC's saved Microsoft sign-in".into()),
    };
    let clear = platform::protect(&encrypted, false)?;
    serde_json::from_slice(&clear)
        .map(Some)
        .map_err(|_| "This PC's saved Microsoft sign-in is invalid".into())
}

fn save_session(root: &Path, session: &Session) -> Result<(), String> {
    let clear =
        serde_json::to_vec(session).map_err(|_| "Could not encode this PC's Microsoft sign-in")?;
    write_atomic(&session_path(root), &platform::protect(&clear, true)?)
}

async fn microsoft_token(
    client: &Client,
    fields: &[(&str, &str)],
) -> Result<MicrosoftToken, String> {
    let mut encoded = Url::parse(TOKEN).map_err(|_| "Invalid Microsoft token URL")?;
    encoded
        .query_pairs_mut()
        .extend_pairs(fields.iter().copied());
    let response = client
        .post(TOKEN)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(encoded.query().unwrap_or_default().to_owned())
        .send()
        .await
        .map_err(|_| "Could not reach Microsoft sign-in")?;
    if !response.status().is_success() {
        return Err(if response.status().is_server_error() {
            "Microsoft sign-in is temporarily unavailable. Try again."
        } else {
            "Microsoft rejected this sign-in. Check the app ID and redirect URI, then reconnect in Settings."
        }.into());
    }
    let token: MicrosoftToken = json_response(response, "Microsoft sign-in").await?;
    if token.access_token.is_empty() {
        return Err("Microsoft did not issue an Xbox sign-in token".into());
    }
    Ok(token)
}

async fn minecraft_session(client: &Client, microsoft_access: &str) -> Result<GameSession, String> {
    let xbox = xbox_request(
        client,
        XBL,
        json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={microsoft_access}")
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }),
    )
    .await?;
    let xsts = xbox_request(
        client,
        XSTS,
        json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbox.token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }),
    )
    .await?;
    let user = xsts
        .display_claims
        .xui
        .first()
        .filter(|user| {
            !user.uhs.is_empty()
                && user.uhs.len() <= 128
                && user
                    .uhs
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() && byte != b';')
        })
        .ok_or("Xbox did not return a valid user identity")?;
    // The hash identifies a user within the XSTS token used for Minecraft sign-in.
    let login = client
        .post(format!("{MINECRAFT}/authentication/login_with_xbox"))
        .json(&json!({ "identityToken": format!("XBL3.0 x={};{}", user.uhs, xsts.token) }))
        .send()
        .await
        .map_err(|_| "Could not reach Minecraft sign-in")?;
    if login.status() == StatusCode::FORBIDDEN {
        return Err("Minecraft denied this sign-in (HTTP 403). Check that Mojang approved Bezalel's app ID and that the account has access.".into());
    }
    let token: MinecraftToken = json_response(login, "Minecraft sign-in").await?;
    if token.access_token.is_empty() || token.expires_in == 0 {
        return Err("Minecraft did not issue a valid game session".into());
    }
    let expires_at = now().saturating_add(token.expires_in);
    let entitlements: Entitlements = json_response(
        client
            .get(format!("{MINECRAFT}/entitlements/mcstore"))
            .bearer_auth(&token.access_token)
            .send()
            .await
            .map_err(|_| "Could not check Minecraft ownership")?,
        "Minecraft ownership",
    )
    .await?;
    if !entitlements
        .items
        .iter()
        .any(|item| matches!(item.name.as_str(), "game_minecraft" | "product_minecraft"))
    {
        return Err(
            "This Microsoft account does not have a Minecraft Java Edition entitlement".into(),
        );
    }
    let profile = client
        .get(format!("{MINECRAFT}/minecraft/profile"))
        .bearer_auth(&token.access_token)
        .send()
        .await
        .map_err(|_| "Could not load your Minecraft profile")?;
    if profile.status() == StatusCode::NOT_FOUND {
        return Err(
            "Create your Minecraft Java profile at minecraft.net, then reconnect in Settings"
                .into(),
        );
    }
    let profile: GameProfile = json_response(profile, "Minecraft profile").await?;
    let id = Uuid::parse_str(&profile.id)
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or("Minecraft returned an invalid profile ID")?;
    if !(1..=16).contains(&profile.name.len())
        || !profile
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("Minecraft returned an invalid profile name".into());
    }
    let xuid = user
        .xuid
        .as_deref()
        .and_then(xuid_string)
        .or_else(|| launch_xuid(&token.access_token))
        .unwrap_or_default();
    Ok(GameSession {
        id: id.simple().to_string(),
        name: profile.name,
        xuid,
        access_token: token.access_token,
        expires_at,
    })
}

async fn xbox_request(client: &Client, endpoint: &str, body: Value) -> Result<XboxToken, String> {
    let response = client
        .post(endpoint)
        .header("x-xbl-contract-version", "1")
        .json(&body)
        .send()
        .await
        .map_err(|_| "Could not reach Xbox sign-in")?;
    if response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN {
        let body: Value = response.json().await.unwrap_or(Value::Null);
        return Err(match body["XErr"].as_u64() {
            Some(2_148_916_233) => {
                "Set up an Xbox profile for this Microsoft account, then reconnect in Settings."
            }
            Some(2_148_916_235) => "Xbox services are unavailable for the region on this account.",
            Some(2_148_916_236 | 2_148_916_237) => {
                "This Microsoft account must complete Xbox age verification."
            }
            Some(2_148_916_238) => {
                "This child account needs an adult family organizer to enable Xbox play."
            }
            _ => "Xbox rejected this sign-in. Check your Xbox account and reconnect in Settings.",
        }
        .into());
    }
    let token: XboxToken = json_response(response, "Xbox sign-in").await?;
    if token.token.is_empty() {
        return Err("Xbox did not issue a sign-in token".into());
    }
    Ok(token)
}

async fn json_response<T: DeserializeOwned>(
    response: Response,
    service: &str,
) -> Result<T, String> {
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "{service} failed (HTTP {}). Try again or reconnect in Settings.",
            status.as_u16()
        ));
    }
    response
        .json()
        .await
        .map_err(|_| format!("{service} returned an invalid response"))
}

fn xuid_string(value: &str) -> Option<String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value != 0)
        .map(|value| value.to_string())
}

fn launch_xuid(access_token: &str) -> Option<String> {
    // This optional launch argument is only a hint from the HTTPS response.
    // Profile identity and ownership come from the authenticated service responses above.
    let payload = access_token.split('.').nth(1)?;
    if payload.len() > 8192 {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    ["xuid", "xid"].into_iter().find_map(|key| {
        claims[key].as_str().and_then(xuid_string).or_else(|| {
            claims[key]
                .as_u64()
                .filter(|value| *value != 0)
                .map(|value| value.to_string())
        })
    })
}
