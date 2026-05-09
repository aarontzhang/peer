use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const SERVICE: &str = "Peer";
const DEVICE_TOKEN_ACCOUNT: &str = "peer-device-token";
const DEFAULT_BACKEND_URL: &str = "https://peer-app.vercel.app";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub signed_in: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDeviceTokenArgs {
    pub token: String,
}

#[derive(Clone)]
pub struct SaasClient {
    client: reqwest::Client,
    base_url: String,
    token: String,
}

impl SaasClient {
    pub fn from_keychain() -> Option<Self> {
        let token = read_device_token()?;
        Some(Self {
            client: reqwest::Client::new(),
            base_url: backend_url(),
            token,
        })
    }

    pub async fn post_json<T>(&self, path: &str, body: Value) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let res = self
            .client
            .post(self.url(path))
            .headers(self.auth_headers()?)
            .json(&body)
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            return Err(anyhow!("Peer backend {status} - {text}"));
        }

        Ok(res.json().await?)
    }

    pub fn post_stream(&self, path: &str) -> Result<reqwest::RequestBuilder> {
        Ok(self
            .client
            .post(self.url(path))
            .headers(self.auth_headers()?)
            .header("accept", "text/event-stream"))
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    fn auth_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        let value = format!("Bearer {}", self.token);
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&value)?);
        Ok(headers)
    }
}

pub fn account_status() -> AccountStatus {
    AccountStatus {
        signed_in: read_device_token().is_some(),
    }
}

pub fn open_login(app: &AppHandle) -> Result<String> {
    let device_id = load_or_create_device_id(app)?;
    let url = format!(
        "{}/api/desktop-login?device_id={}&redirect_uri={}",
        backend_url().trim_end_matches('/'),
        percent_encode(&device_id),
        percent_encode("peer://auth")
    );

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .context("opening browser login")?;

    #[cfg(not(target_os = "macos"))]
    return Err(anyhow!("desktop login is only supported on macOS"));

    Ok(url)
}

pub fn set_device_token(args: SetDeviceTokenArgs) -> Result<()> {
    let token = args.token.trim();
    if token.is_empty() {
        return Err(anyhow!("device token is empty"));
    }
    keyring::Entry::new(SERVICE, DEVICE_TOKEN_ACCOUNT)?
        .set_password(token)
        .context("storing device token in Keychain")
}

pub fn sign_out() -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, DEVICE_TOKEN_ACCOUNT)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err).context("removing device token from Keychain"),
    }
}

fn read_device_token() -> Option<String> {
    keyring::Entry::new(SERVICE, DEVICE_TOKEN_ACCOUNT)
        .and_then(|entry| entry.get_password())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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

fn load_or_create_device_id(app: &AppHandle) -> Result<String> {
    let path = device_id_path(app)?;
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(id) = value
                .get("deviceId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Ok(id.to_string());
            }
        }
    }

    let id = Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({ "deviceId": id }))?,
    )?;
    Ok(id)
}

fn device_id_path(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .context("resolving app data directory")?;
    Ok(dir.join("device.json"))
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
