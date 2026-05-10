use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use parking_lot::Mutex;
use rand::{rngs::OsRng, RngCore};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

const SERVICE: &str = "Peer";
const SESSION_ACCOUNT: &str = "peer-session";
const LEGACY_DEVICE_TOKEN_ACCOUNT: &str = "peer-device-token";
const DEFAULT_BACKEND_URL: &str = "https://peer-wheat.vercel.app";
const DEFAULT_SUPABASE_URL: &str = "https://hmkpgxlfxwztficbuktj.supabase.co";

/// CSRF state generated when launching the OAuth browser leg, consumed when
/// the deep-link handler fires. Static because the deep-link callback runs
/// outside any per-request context — the browser hop is a side-channel.
static PENDING_STATE: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub signed_in: bool,
    pub email: Option<String>,
}

#[derive(Clone)]
pub struct SaasClient {
    client: reqwest::Client,
    base_url: String,
    session: Arc<Mutex<Session>>,
    app: AppHandle,
}

impl SaasClient {
    pub async fn from_keychain(app: AppHandle) -> Option<Self> {
        let mut s = read_session()?;
        match refresh_if_needed(&mut s, false).await {
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(?err, "proactive refresh failed; clearing session");
                let _ = clear_session();
                let _ = app.emit(
                    "auth:changed",
                    json!({ "signedIn": false, "email": null }),
                );
                return None;
            }
        }
        Some(Self {
            client: reqwest::Client::new(),
            base_url: backend_url(),
            session: Arc::new(Mutex::new(s)),
            app,
        })
    }

    pub async fn post_json<T>(&self, path: &str, body: Value) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let url = self.url(path);
        let send = |access: String| {
            self.client
                .post(&url)
                .headers(bearer_headers(&access).unwrap_or_default())
                .json(&body)
                .send()
        };

        let access = self.session.lock().access_token.clone();
        let res = send(access).await?;
        let res = if res.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.force_refresh().await?;
            let access = self.session.lock().access_token.clone();
            send(access).await?
        } else {
            res
        };

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                let _ = clear_session();
                let _ = self
                    .app
                    .emit("auth:changed", json!({ "signedIn": false, "email": null }));
            }
            return Err(anyhow!("Peer backend {status} - {text}"));
        }

        Ok(res.json().await?)
    }

    /// Streaming variant. Does NOT retry on 401 — the SSE consumer in
    /// `pipeline/analyze.rs` reads the response body incrementally and
    /// rewinding the stream is more invasive than it's worth. A 401 here
    /// surfaces as a backend error; a successful refresh on the next
    /// recording recovers the session.
    pub fn post_stream(&self, path: &str) -> Result<reqwest::RequestBuilder> {
        let access = self.session.lock().access_token.clone();
        Ok(self
            .client
            .post(self.url(path))
            .headers(bearer_headers(&access)?)
            .header("accept", "text/event-stream"))
    }

    async fn force_refresh(&self) -> Result<()> {
        let mut s = self.session.lock().clone();
        match refresh_if_needed(&mut s, true).await {
            Ok(_) => {
                *self.session.lock() = s;
                Ok(())
            }
            Err(err) => {
                let _ = clear_session();
                let _ = self
                    .app
                    .emit("auth:changed", json!({ "signedIn": false, "email": null }));
                Err(err)
            }
        }
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn bearer_headers(token: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    let value = format!("Bearer {token}");
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&value)?);
    Ok(headers)
}

pub fn account_status() -> AccountStatus {
    let Some(session) = read_session() else {
        return AccountStatus {
            signed_in: false,
            email: None,
        };
    };
    AccountStatus {
        signed_in: true,
        email: decode_email_from_jwt(&session.access_token),
    }
}

pub fn open_login(_app: &AppHandle) -> Result<String> {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf);
    *PENDING_STATE.lock() = Some(nonce.clone());

    let supabase = supabase_url();
    // Carry the CSRF nonce through `redirect_to` rather than the OAuth `state`
    // param. Supabase wants to own `state` for its own implicit-flow validation;
    // passing our own value triggers a bad_oauth_state on the callback leg.
    let callback = format!(
        "{}/api/auth-callback?nonce={}",
        backend_url().trim_end_matches('/'),
        percent_encode(&nonce),
    );
    let url = format!(
        "{}/auth/v1/authorize?provider=google&redirect_to={}&flow_type=implicit",
        supabase.trim_end_matches('/'),
        percent_encode(&callback),
    );

    tracing::info!(%url, "opening google sign-in");
    eprintln!("[peer] sign-in URL: {url}");

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .context("opening browser login")?;

    #[cfg(not(target_os = "macos"))]
    return Err(anyhow!("desktop login is only supported on macOS"));

    Ok(url)
}

/// Parse the `peer://auth#…` URL produced by the Vercel callback page.
/// Tokens travel in the URL fragment — Supabase implicit flow keeps them
/// out of any server log or `Referer` header, so we only ever see them here.
pub fn handle_deep_link(app: &AppHandle, raw_url: &str) -> Result<()> {
    let parsed = url::Url::parse(raw_url).context("parsing deep link URL")?;
    if parsed.scheme() != "peer" {
        return Err(anyhow!("unexpected deep link scheme: {}", parsed.scheme()));
    }
    let host_or_path = parsed.host_str().unwrap_or("").to_string() + parsed.path();
    if !host_or_path.contains("auth") {
        return Err(anyhow!("unexpected deep link target: {}", host_or_path));
    }

    let fragment = parsed.fragment().unwrap_or("");
    let mut access_token: Option<String> = None;
    let mut refresh_token: Option<String> = None;
    let mut expires_in: Option<i64> = None;
    let mut expires_at: Option<i64> = None;
    let mut nonce_param: Option<String> = None;
    let mut error: Option<String> = None;

    // Nonce arrives via the redirect_to query string; tokens via the fragment.
    for (k, v) in parsed.query_pairs() {
        if k.as_ref() == "nonce" {
            nonce_param = Some(v.into_owned());
        }
    }
    for (k, v) in url::form_urlencoded::parse(fragment.as_bytes()) {
        match k.as_ref() {
            "access_token" => access_token = Some(v.into_owned()),
            "refresh_token" => refresh_token = Some(v.into_owned()),
            "expires_in" => expires_in = v.parse().ok(),
            "expires_at" => expires_at = v.parse().ok(),
            "nonce" => nonce_param = Some(v.into_owned()),
            "error" | "error_description" => error = Some(v.into_owned()),
            _ => {}
        }
    }

    if let Some(err) = error {
        tracing::warn!(error = %err, "OAuth callback returned error");
        let _ = app.emit(
            "auth:changed",
            json!({ "signedIn": false, "email": null, "error": err }),
        );
        return Ok(());
    }

    let expected = PENDING_STATE.lock().take();
    match (expected.as_deref(), nonce_param.as_deref()) {
        (Some(expected), Some(got)) if expected == got => {}
        _ => {
            tracing::warn!("deep link nonce mismatch; ignoring");
            let _ = app.emit(
                "auth:changed",
                json!({
                    "signedIn": false,
                    "email": null,
                    "error": "Sign-in nonce mismatch — open Settings and try again.",
                }),
            );
            return Ok(());
        }
    }

    let Some(access_token) = access_token else {
        tracing::warn!("deep link missing access_token");
        return Ok(());
    };
    let Some(refresh_token) = refresh_token else {
        tracing::warn!("deep link missing refresh_token");
        return Ok(());
    };

    let computed_expires_at = expires_at.unwrap_or_else(|| now_secs() + expires_in.unwrap_or(3600));
    let session = Session {
        access_token,
        refresh_token,
        expires_at: computed_expires_at,
    };
    write_session(&session)?;

    let email = decode_email_from_jwt(&session.access_token);
    let _ = app.emit(
        "auth:changed",
        json!({ "signedIn": true, "email": email }),
    );
    let _ = crate::reveal_result_window(app, true);
    Ok(())
}

pub fn sign_out(app: &AppHandle) -> Result<()> {
    clear_session()?;
    let _ = app.emit("auth:changed", json!({ "signedIn": false, "email": null }));
    Ok(())
}

fn read_session() -> Option<Session> {
    let raw = match keyring::Entry::new(SERVICE, SESSION_ACCOUNT)
        .and_then(|entry| entry.get_password())
    {
        Ok(s) => s,
        Err(_) => {
            // Silent migration from the old device-token model.
            let _ = legacy_clear();
            return None;
        }
    };
    serde_json::from_str::<Session>(&raw).ok()
}

fn write_session(s: &Session) -> Result<()> {
    let json = serde_json::to_string(s)?;
    keyring::Entry::new(SERVICE, SESSION_ACCOUNT)?
        .set_password(&json)
        .context("storing session in Keychain")?;
    let _ = legacy_clear();
    Ok(())
}

fn clear_session() -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, SESSION_ACCOUNT)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err).context("removing session from Keychain"),
    }
}

fn legacy_clear() -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, LEGACY_DEVICE_TOKEN_ACCOUNT)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err).context("removing legacy device token"),
    }
}

/// Refresh the session against Supabase if it's near expiry (or `force`).
/// Returns true when a refresh actually occurred. On 4xx the session is
/// considered dead and the error is bubbled up; the caller decides whether
/// to clear and emit `auth:changed`.
async fn refresh_if_needed(session: &mut Session, force: bool) -> Result<bool> {
    if !force && session.expires_at - now_secs() > 300 {
        return Ok(false);
    }
    let anon = anon_key().ok_or_else(|| anyhow!("SUPABASE_ANON_KEY not configured"))?;
    let url = format!(
        "{}/auth/v1/token?grant_type=refresh_token",
        supabase_url().trim_end_matches('/')
    );
    let res = reqwest::Client::new()
        .post(&url)
        .header("apikey", anon.clone())
        .header("Authorization", format!("Bearer {anon}"))
        .json(&json!({ "refresh_token": session.refresh_token }))
        .send()
        .await?;

    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if status.is_client_error() {
        return Err(anyhow!("refresh rejected: {status} — {body}"));
    }
    if !status.is_success() {
        return Err(anyhow!("refresh failed: {status} — {body}"));
    }

    let v: Value =
        serde_json::from_str(&body).context("parsing refresh response")?;
    let access = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("refresh missing access_token"))?
        .to_string();
    let refresh = v
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| session.refresh_token.clone());
    let expires_in = v.get("expires_in").and_then(|x| x.as_i64()).unwrap_or(3600);

    session.access_token = access;
    session.refresh_token = refresh;
    session.expires_at = now_secs() + expires_in;
    write_session(session)?;
    Ok(true)
}

fn decode_email_from_jwt(jwt: &str) -> Option<String> {
    let mut parts = jwt.split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload_b64))
        .ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("email")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn backend_url() -> String {
    std::env::var("PEER_BACKEND_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            option_env!("PEER_BACKEND_URL")
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| DEFAULT_BACKEND_URL.to_string())
}

fn supabase_url() -> String {
    std::env::var("SUPABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            option_env!("SUPABASE_URL")
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| DEFAULT_SUPABASE_URL.to_string())
}

fn anon_key() -> Option<String> {
    std::env::var("SUPABASE_ANON_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            option_env!("SUPABASE_ANON_KEY")
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
        })
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
