use std::{
    net::{SocketAddr, TcpListener as StdTcpListener},
    sync::Arc,
    time::Duration,
};

#[cfg(debug_assertions)]
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use parking_lot::Mutex;
use rand::{rngs::OsRng, RngCore};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[cfg(not(debug_assertions))]
const SERVICE: &str = "Peer";
#[cfg(not(debug_assertions))]
const SESSION_ACCOUNT: &str = "peer-session";
#[cfg(not(debug_assertions))]
const LEGACY_DEVICE_TOKEN_ACCOUNT: &str = "peer-device-token";
const DEFAULT_BACKEND_URL: &str = "https://peer-wheat.vercel.app";
const DEFAULT_SUPABASE_URL: &str = "https://hmkpgxlfxwztficbuktj.supabase.co";
const LOOPBACK_AUTH_ADDR: &str = "127.0.0.1:17643";
const LOOPBACK_AUTH_TIMEOUT: Duration = Duration::from_secs(10 * 60);

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
                let _ = app.emit("auth:changed", json!({ "signedIn": false, "email": null }));
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

pub fn open_login(app: &AppHandle) -> Result<String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        return Err(anyhow!("desktop login is only supported on macOS"));
    }

    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf);
    let listener = bind_loopback_auth_listener()?;
    *PENDING_STATE.lock() = Some(nonce.clone());

    let supabase = supabase_url();
    // Carry the CSRF nonce through `redirect_to` rather than the OAuth `state`
    // param. Supabase wants to own `state` for its own implicit-flow validation;
    // passing our own value triggers a bad_oauth_state on the callback leg. Use
    // a loopback callback so stale macOS peer:// handlers cannot steal the
    // OAuth completion from the currently running app.
    let callback = format!(
        "http://{}/auth-callback?nonce={}",
        LOOPBACK_AUTH_ADDR,
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
    {
        if let Err(err) = std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .context("opening browser login")
        {
            *PENDING_STATE.lock() = None;
            return Err(err);
        }
        spawn_loopback_auth_server(app.clone(), listener);
    }

    Ok(url)
}

fn bind_loopback_auth_listener() -> Result<StdTcpListener> {
    let listener = StdTcpListener::bind(LOOPBACK_AUTH_ADDR)
        .with_context(|| format!("sign-in helper port {LOOPBACK_AUTH_ADDR} is busy"))?;
    listener
        .set_nonblocking(true)
        .context("configuring sign-in helper listener")?;
    Ok(listener)
}

fn spawn_loopback_auth_server(app: AppHandle, listener: StdTcpListener) {
    tauri::async_runtime::spawn(async move {
        let listener = match TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(err) => {
                tracing::warn!(?err, "starting sign-in helper listener failed");
                let mut pending = PENDING_STATE.lock();
                if pending.take().is_some() {
                    let _ = app.emit(
                        "auth:changed",
                        json!({
                            "signedIn": false,
                            "email": null,
                            "error": "Could not start sign-in helper — open Settings and try again.",
                        }),
                    );
                }
                return;
            }
        };

        let serve = async {
            loop {
                let (stream, addr) = listener.accept().await?;
                match handle_loopback_auth_request(&app, stream, addr).await {
                    Ok(done) if done => break,
                    Ok(_) => {}
                    Err(err) => tracing::warn!(?err, "loopback auth request failed"),
                }
            }
            Ok::<(), anyhow::Error>(())
        };

        if tokio::time::timeout(LOOPBACK_AUTH_TIMEOUT, serve)
            .await
            .is_err()
        {
            let mut pending = PENDING_STATE.lock();
            if pending.take().is_some() {
                let _ = app.emit(
                    "auth:changed",
                    json!({
                        "signedIn": false,
                        "email": null,
                        "error": "Sign-in timed out — open Settings and try again.",
                    }),
                );
            }
        }
    });
}

async fn handle_loopback_auth_request(
    app: &AppHandle,
    mut stream: TcpStream,
    addr: SocketAddr,
) -> Result<bool> {
    if !addr.ip().is_loopback() {
        write_http_response(
            &mut stream,
            403,
            "Forbidden",
            "text/plain; charset=utf-8",
            "Forbidden",
        )
        .await?;
        return Ok(false);
    }

    let request = read_http_request(&mut stream).await?;
    let done = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/auth-callback") => {
            let html = loopback_callback_page();
            write_http_response(&mut stream, 200, "OK", "text/html; charset=utf-8", &html).await?;
            false
        }
        ("POST", "/auth-token") => {
            let payload: LoopbackAuthPayload =
                serde_json::from_slice(&request.body).context("parsing loopback auth payload")?;
            let hash = payload.hash.trim();
            let fragment = hash.strip_prefix('#').unwrap_or(hash);
            let raw_url = format!(
                "{}://auth?nonce={}#{}",
                deep_link_scheme(),
                percent_encode(&payload.nonce),
                fragment
            );
            handle_deep_link(app, &raw_url)?;
            write_http_response(
                &mut stream,
                200,
                "OK",
                "application/json; charset=utf-8",
                r#"{"ok":true}"#,
            )
            .await?;
            true
        }
        _ => {
            write_http_response(
                &mut stream,
                404,
                "Not Found",
                "text/plain; charset=utf-8",
                "Not Found",
            )
            .await?;
            false
        }
    };
    Ok(done)
}

#[derive(Debug, Deserialize)]
struct LoopbackAuthPayload {
    nonce: String,
    hash: String,
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

async fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let n = stream
            .read(&mut tmp)
            .await
            .context("reading auth callback")?;
        if n == 0 {
            return Err(anyhow!("auth callback connection closed before headers"));
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > 16 * 1024 {
            return Err(anyhow!("auth callback headers too large"));
        }
    };

    let headers = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = headers.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("auth callback missing request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| anyhow!("auth callback missing method"))?
        .to_string();
    let target = parts
        .next()
        .ok_or_else(|| anyhow!("auth callback missing path"))?;
    let path = target.split('?').next().unwrap_or(target).to_string();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value
                    .trim()
                    .parse()
                    .context("parsing auth callback content-length")?;
            }
        }
    }
    if content_length > 16 * 1024 {
        return Err(anyhow!("auth callback body too large"));
    }

    let body_start = header_end + 4;
    let mut body = buf.get(body_start..).unwrap_or_default().to_vec();
    while body.len() < content_length {
        let n = stream
            .read(&mut tmp)
            .await
            .context("reading auth callback body")?;
        if n == 0 {
            return Err(anyhow!("auth callback connection closed before body"));
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    Ok(HttpRequest { method, path, body })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\ncache-control: no-store\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .context("writing auth callback response")?;
    Ok(())
}

fn loopback_callback_page() -> String {
    r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Signing in to Peer</title>
  <style>
    body{margin:0;font:14px/1.5 -apple-system,BlinkMacSystemFont,"SF Pro Text",Segoe UI,sans-serif;background:#151515;color:#f5f2ec;display:grid;place-items:center;min-height:100vh}
    main{width:min(420px,calc(100vw - 40px));border:1px solid #3a3833;border-radius:12px;padding:24px;background:#20201e;text-align:center}
    h1{font-size:22px;margin:0 0 8px}
    p{color:#b9b3a8;margin:0}
    code{display:block;white-space:pre-wrap;word-break:break-all;background:#111;padding:12px;border-radius:8px;margin-top:14px;color:#f08a8a;text-align:left}
  </style>
</head>
<body>
  <main>
    <h1>Signing in to Peer</h1>
    <p id="status">Returning you to the app...</p>
    <div id="error"></div>
  </main>
  <script>
    (async function () {
      var status = document.getElementById('status');
      var errorBox = document.getElementById('error');
      var nonce = new URLSearchParams(window.location.search).get('nonce') || '';
      var hash = window.location.hash || '';
      if (!hash) {
        status.textContent = 'No sign-in tokens were returned.';
        return;
      }
      try {
        var res = await fetch('/auth-token', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ nonce: nonce, hash: hash })
        });
        if (!res.ok) throw new Error('Peer returned HTTP ' + res.status);
        if (hash.indexOf('error=') !== -1) {
          status.textContent = 'Sign-in failed. Return to Peer to try again.';
        } else {
          status.textContent = 'Signed in. You can close this tab.';
        }
      } catch (err) {
        status.textContent = 'Could not return sign-in to Peer.';
        var msg = String(err && err.message ? err.message : err);
        errorBox.innerHTML = '<code>' + msg.replace(/[&<>]/g, function (c) { return ({'&':'&amp;','<':'&lt;','>':'&gt;'})[c]; }) + '</code>';
      }
    })();
  </script>
</body>
</html>"#
        .to_string()
}

/// Parse the `peer://auth#…` URL produced by the Vercel callback page.
/// Tokens travel in the URL fragment — Supabase implicit flow keeps them
/// out of any server log or `Referer` header, so we only ever see them here.
pub fn handle_deep_link(app: &AppHandle, raw_url: &str) -> Result<()> {
    let expected = PENDING_STATE.lock().clone();
    let outcome = parse_auth_deep_link(raw_url, expected.as_deref(), deep_link_scheme())?;
    if outcome.consumes_pending_state() {
        let _ = PENDING_STATE.lock().take();
    }

    match outcome {
        AuthLinkOutcome::SignedIn(session) => {
            write_session(&session)?;
            let email = decode_email_from_jwt(&session.access_token);
            let _ = app.emit("auth:changed", json!({ "signedIn": true, "email": email }));
            let _ = crate::reveal_result_window(app, true);
        }
        AuthLinkOutcome::NoAccount => {
            let _ = app.emit(
                "auth:changed",
                json!({
                    "signedIn": false,
                    "email": null,
                    "reason": "no_account",
                }),
            );
        }
        AuthLinkOutcome::OAuthError(raw_msg) | AuthLinkOutcome::Rejected(raw_msg) => {
            let _ = app.emit(
                "auth:changed",
                json!({ "signedIn": false, "email": null, "error": raw_msg }),
            );
        }
    }
    Ok(())
}

#[derive(Debug)]
enum AuthLinkOutcome {
    SignedIn(Session),
    NoAccount,
    OAuthError(String),
    Rejected(String),
}

impl AuthLinkOutcome {
    fn consumes_pending_state(&self) -> bool {
        matches!(self, Self::SignedIn(_) | Self::Rejected(_))
    }
}

fn parse_auth_deep_link(
    raw_url: &str,
    expected_nonce: Option<&str>,
    expected_scheme: &str,
) -> Result<AuthLinkOutcome> {
    let parsed = url::Url::parse(raw_url).context("parsing deep link URL")?;
    if parsed.scheme() != expected_scheme {
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
    let mut error_code: Option<String> = None;
    let mut error_description: Option<String> = None;

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
            "error" => error = Some(v.into_owned()),
            "error_code" => error_code = Some(v.into_owned()),
            "error_description" => error_description = Some(v.into_owned()),
            _ => {}
        }
    }

    if error.is_some() || error_code.is_some() || error_description.is_some() {
        // Supabase returns `error_code=signup_disabled` (and an `access_denied`
        // error) when OAuth is attempted with an unrecognised account while
        // signups are turned off in the dashboard. Surface that as a distinct
        // reason so the UI can show a friendly "no account" path instead of
        // the generic error message.
        let desc_lower = error_description.as_deref().unwrap_or("").to_lowercase();
        let is_no_account = error_code.as_deref() == Some("signup_disabled")
            || desc_lower.contains("signups not allowed")
            || desc_lower.contains("signup is disabled");

        let raw_msg = error_description
            .clone()
            .or(error_code.clone())
            .or(error.clone())
            .unwrap_or_else(|| "unknown error".to_string());

        tracing::warn!(error = %raw_msg, no_account = is_no_account, "OAuth callback returned error");

        if is_no_account {
            return Ok(AuthLinkOutcome::NoAccount);
        } else {
            return Ok(AuthLinkOutcome::OAuthError(raw_msg));
        }
    }

    match (expected_nonce, nonce_param.as_deref()) {
        (Some(expected), Some(got)) if expected == got => {}
        _ => {
            tracing::warn!("deep link nonce mismatch; ignoring");
            return Ok(AuthLinkOutcome::Rejected(
                "Sign-in nonce mismatch — open Settings and try again.".into(),
            ));
        }
    }

    let Some(access_token) = access_token else {
        tracing::warn!("deep link missing access_token");
        return Ok(AuthLinkOutcome::Rejected(
            "Sign-in returned no tokens — try again.".into(),
        ));
    };
    let Some(refresh_token) = refresh_token else {
        tracing::warn!("deep link missing refresh_token");
        return Ok(AuthLinkOutcome::Rejected(
            "Sign-in returned no tokens — try again.".into(),
        ));
    };

    let computed_expires_at = expires_at.unwrap_or_else(|| now_secs() + expires_in.unwrap_or(3600));
    Ok(AuthLinkOutcome::SignedIn(Session {
        access_token,
        refresh_token,
        expires_at: computed_expires_at,
    }))
}

pub fn sign_out(app: &AppHandle) -> Result<()> {
    clear_session()?;
    let _ = app.emit("auth:changed", json!({ "signedIn": false, "email": null }));
    Ok(())
}

fn read_session() -> Option<Session> {
    #[cfg(debug_assertions)]
    {
        return read_dev_session();
    }

    #[cfg(not(debug_assertions))]
    {
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
}

fn write_session(s: &Session) -> Result<()> {
    #[cfg(debug_assertions)]
    {
        return write_dev_session(s);
    }

    #[cfg(not(debug_assertions))]
    {
        let json = serde_json::to_string(s)?;
        keyring::Entry::new(SERVICE, SESSION_ACCOUNT)?
            .set_password(&json)
            .context("storing session in Keychain")?;
        let _ = legacy_clear();
        Ok(())
    }
}

fn clear_session() -> Result<()> {
    #[cfg(debug_assertions)]
    {
        let path = dev_session_path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err).context("removing dev session file"),
        }
    }

    #[cfg(not(debug_assertions))]
    {
        let entry = keyring::Entry::new(SERVICE, SESSION_ACCOUNT)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err).context("removing session from Keychain"),
        }
    }
}

#[cfg(not(debug_assertions))]
fn legacy_clear() -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, LEGACY_DEVICE_TOKEN_ACCOUNT)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err).context("removing legacy device token"),
    }
}

#[cfg(debug_assertions)]
fn dev_session_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.aaronzhang.peer")
        .join("peer-session.json")
}

#[cfg(debug_assertions)]
fn read_dev_session() -> Option<Session> {
    let bytes = std::fs::read(dev_session_path()).ok()?;
    serde_json::from_slice::<Session>(&bytes).ok()
}

#[cfg(debug_assertions)]
fn write_dev_session(s: &Session) -> Result<()> {
    let path = dev_session_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec(s)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(&json)?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, json)?;
        Ok(())
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

    let v: Value = serde_json::from_str(&body).context("parsing refresh response")?;
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

/// `peer-dev://` in debug builds keeps a co-installed prod /Applications/Peer.app
/// from intercepting OAuth deep links handed back by the browser.
fn deep_link_scheme() -> &'static str {
    #[cfg(debug_assertions)]
    {
        "peer-dev"
    }
    #[cfg(not(debug_assertions))]
    {
        "peer"
    }
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

#[cfg(test)]
mod tests {
    use super::{decode_email_from_jwt, parse_auth_deep_link, AuthLinkOutcome};
    use base64::Engine;
    use serde_json::json;

    fn jwt_with_email(email: &str) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(json!({ "email": email }).to_string());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn deep_link_parser_accepts_valid_tokens_and_nonce() {
        let access = jwt_with_email("user@example.com");
        let raw = format!(
            "peer-dev://auth?nonce=abc#access_token={access}&refresh_token=refresh&expires_at=12345"
        );

        let outcome = parse_auth_deep_link(&raw, Some("abc"), "peer-dev").unwrap();

        let AuthLinkOutcome::SignedIn(session) = outcome else {
            panic!("expected signed-in outcome");
        };
        assert_eq!(session.access_token, access);
        assert_eq!(session.refresh_token, "refresh");
        assert_eq!(session.expires_at, 12345);
        assert_eq!(
            decode_email_from_jwt(&session.access_token).as_deref(),
            Some("user@example.com")
        );
    }

    #[test]
    fn deep_link_parser_maps_signup_disabled_to_no_account() {
        let raw = "peer-dev://auth?nonce=abc#error=access_denied&error_code=signup_disabled&error_description=Signups%20not%20allowed";

        let outcome = parse_auth_deep_link(raw, Some("abc"), "peer-dev").unwrap();

        assert!(matches!(outcome, AuthLinkOutcome::NoAccount));
        assert!(!outcome.consumes_pending_state());
    }

    #[test]
    fn deep_link_parser_rejects_nonce_mismatch_and_missing_tokens() {
        let access = jwt_with_email("user@example.com");
        let mismatch = format!(
            "peer-dev://auth?nonce=wrong#access_token={access}&refresh_token=refresh&expires_in=60"
        );
        let missing = "peer-dev://auth?nonce=abc#access_token=only-access";

        let mismatch = parse_auth_deep_link(&mismatch, Some("abc"), "peer-dev").unwrap();
        let missing = parse_auth_deep_link(missing, Some("abc"), "peer-dev").unwrap();

        assert!(
            matches!(mismatch, AuthLinkOutcome::Rejected(ref msg) if msg.contains("nonce mismatch"))
        );
        assert!(mismatch.consumes_pending_state());
        assert!(matches!(missing, AuthLinkOutcome::Rejected(ref msg) if msg.contains("no tokens")));
        assert!(missing.consumes_pending_state());
    }

    #[test]
    fn deep_link_parser_rejects_unexpected_scheme_or_target() {
        let bad_scheme = parse_auth_deep_link("peer://auth#x=1", Some("abc"), "peer-dev");
        let bad_target = parse_auth_deep_link("peer-dev://settings#x=1", Some("abc"), "peer-dev");

        assert!(bad_scheme.is_err());
        assert!(bad_target.is_err());
    }
}
