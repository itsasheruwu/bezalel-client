use crate::{
    state::Result,
    studio::{oauth_callback_at, platform},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::{Client, Method, RequestBuilder, Url};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{net::TcpListener, time::timeout};
use uuid::Uuid;

const CALLBACK_PATH: &str = "/bezalel/signin";
const MAX_RESPONSE_BYTES: usize = 32 * 1024;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub avatar: String,
    pub setup_complete: bool,
    #[serde(default)]
    pub can_invite: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignIn {
    pub token: String,
    pub expires_at: u64,
    pub profile: Profile,
}

#[derive(Deserialize, Serialize)]
pub struct AccessCode {
    pub code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Authorization {
    authorize_url: String,
}

pub fn configured_service() -> Result<(&'static str, &'static str)> {
    let url = option_env!("BEZALEL_SYNC_URL").unwrap_or_default();
    let key = option_env!("BEZALEL_SYNC_SIGNING_PUBLIC_KEY").unwrap_or_default();
    if url.is_empty() || key.is_empty() {
        return Err("This build uses local packs until hosted sharing is configured.".into());
    }
    service_url(url)?;
    bezalel_pack_sync::parse_public_key(key)?;
    Ok((url, key))
}

fn service_url(url: &str) -> Result<Url> {
    let mut base = Url::parse(url).map_err(|_| "Invalid Bezalel sharing address.")?;
    if !(base.scheme() == "https"
        || base.scheme() == "http" && base.host_str() == Some("127.0.0.1"))
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err("Use an HTTPS address for hosted Bezalel sharing.".into());
    }
    let path = format!("{}/", base.path().trim_end_matches('/'));
    base.set_path(&path);
    Ok(base)
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_url_value() -> Result<String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "Could not secure the sign-in request.")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn authorization_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| "Invalid Microsoft sign-in address.")?;
    if url.scheme() != "https"
        || url.host_str() != Some("login.microsoftonline.com")
        || !url.path().ends_with("/oauth2/v2.0/authorize")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("The sharing service returned an unexpected Microsoft sign-in address.".into());
    }
    Ok(url)
}

pub struct HostedApi {
    base: Url,
    http: Client,
}

impl HostedApi {
    pub fn new(url: &str) -> Result<Self> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "Could not initialize Bezalel sharing.")?;
        Ok(Self {
            base: service_url(url)?,
            http,
        })
    }

    fn request(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let endpoint = self
            .base
            .join(path)
            .map_err(|_| "Invalid Bezalel sharing endpoint.")?;
        Ok(self.http.request(method, endpoint))
    }

    async fn response(&self, request: RequestBuilder) -> Result<reqwest::Response> {
        let response = request
            .send()
            .await
            .map_err(|_| "Could not reach Bezalel sharing. Try again shortly.")?;
        match response.status().as_u16() {
            200..=299 => Ok(response),
            401 => Err("Your Bezalel session expired. Sign in again.".into()),
            429 => Err("Too many sign-in requests. Try again in a minute.".into()),
            503 => Err("Microsoft sign-in is still being set up on this Bezalel service.".into()),
            _ => Err("Bezalel sharing could not complete the request.".into()),
        }
    }

    async fn json<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        Self::read_json(self.response(request).await?).await
    }

    async fn read_json<T: DeserializeOwned>(mut response: reqwest::Response) -> Result<T> {
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Could not read the sharing service response.")?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err("The sharing service response was too large.".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| "The sharing service returned an invalid response.".into())
    }

    pub async fn sign_in(&self) -> Result<SignIn> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| "Could not open the local sign-in callback.")?;
        let port = listener
            .local_addr()
            .map_err(|_| "Could not open the local sign-in callback.")?
            .port();
        let callback_url = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let state = random_url_value()?;
        let verifier = random_url_value()?;
        let challenge =
            URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, verifier.as_bytes()));
        let authorization: Authorization = self
            .json(
                self.request(Method::POST, "v1/auth/microsoft/start")?
                    .json(&json!({
                        "callbackUrl": callback_url,
                        "challenge": challenge,
                        "state": state
                    })),
            )
            .await?;
        platform::open(authorization_url(&authorization.authorize_url)?.as_str())?;
        let (code, _) = timeout(
            Duration::from_secs(180),
            oauth_callback_at(listener, &state, CALLBACK_PATH),
        )
        .await
        .map_err(|_| "Microsoft sign-in timed out. Open Bezalel to try again.")??;
        let session: SignIn = self
            .json(
                self.request(Method::POST, "v1/auth/microsoft/exchange")?
                    .json(&json!({
                        "code": code,
                        "verifier": verifier
                    })),
            )
            .await?;
        checked_session(session)
    }

    pub async fn redeem_code(&self, code: &str) -> Result<SignIn> {
        if code.is_empty() || code.len() > 128 {
            return Err("Enter the Bezalel access code you were given.".into());
        }
        let request = self
            .request(Method::POST, "v1/auth/code/redeem")?
            .json(&json!({ "code": code }));
        let response = request
            .send()
            .await
            .map_err(|_| "Could not reach Bezalel sharing. Try again shortly.")?;
        match response.status().as_u16() {
            200..=299 => {
                let session: SignIn = Self::read_json(response).await?;
                checked_session(session)
            }
            400 | 401 | 403 => Err("That Bezalel access code could not be accepted.".into()),
            429 => Err("Too many sign-in requests. Try again in a minute.".into()),
            _ => Err("Bezalel sharing could not complete sign-in.".into()),
        }
    }

    pub async fn create_access_code(&self, token: &str) -> Result<AccessCode> {
        self.json(
            self.request(Method::POST, "v1/auth/code/invite")?
                .bearer_auth(token)
                .json(&json!({})),
        )
        .await
    }

    pub async fn profile(&self, token: &str) -> Result<Profile> {
        self.json(
            self.request(Method::GET, "v1/me/profile")?
                .bearer_auth(token),
        )
        .await
    }

    pub async fn save_profile(&self, token: &str, name: &str, avatar: &str) -> Result<Profile> {
        self.json(
            self.request(Method::PATCH, "v1/me/profile")?
                .bearer_auth(token)
                .json(&json!({ "name": name, "avatar": avatar })),
        )
        .await
    }

    pub async fn logout(&self, token: &str) -> Result<()> {
        self.response(
            self.request(Method::POST, "v1/auth/logout")?
                .bearer_auth(token),
        )
        .await?;
        Ok(())
    }
}

fn checked_session(session: SignIn) -> Result<SignIn> {
    if session.expires_at <= now()
        || session.token.len() != 64
        || !session.token.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("The sharing service returned an invalid session.".into());
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_sign_in_opens_the_microsoft_authorization_host() {
        assert!(authorization_url(
            "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?state=abc"
        )
        .is_ok());
        for value in [
            "http://login.microsoftonline.com/consumers/oauth2/v2.0/authorize",
            "https://login.microsoftonline.com.example.org/consumers/oauth2/v2.0/authorize",
            "https://login.microsoftonline.com@evil.example/consumers/oauth2/v2.0/authorize",
            "https://login.microsoftonline.com/consumers/oauth2/v2.0/token",
        ] {
            assert!(authorization_url(value).is_err(), "{value}");
        }
    }
}
