//! OAuth login for HTTP MCP servers. The protocol work (OAuth 2.1 authorization code + PKCE,
//! resource indicators, protected resource and authorization server metadata discovery with
//! the OAuth 2.0 / MCP 2025-03-26 fallbacks, dynamic client registration, token refresh) is
//! done by `rmcp`'s auth module. This file holds what is minerve's own: where logins are
//! stored, the browser, and the localhost page the browser comes back to.
//!
//! Logins are kept in `~/.minerve/mcp_auth.json` (owner-only), keyed by the MCP server URL.

use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession, CredentialStore, StoredCredentials,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const AUTH_FILE_PATH: &str = ".minerve/mcp_auth.json";
const CALLBACK_PATH: &str = "/callback";
/// How long the browser login may take before it is given up.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const CLIENT_NAME: &str = "minerve";

fn auth_file() -> PathBuf {
    dirs::home_dir().expect("no home directory").join(AUTH_FILE_PATH)
}

fn read_logins() -> Result<BTreeMap<String, StoredCredentials>, String> {
    let path = auth_file();
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|err| format!("{}: {}", path.display(), err))?;
    serde_json::from_str(&text).map_err(|err| format!("{}: {}", path.display(), err))
}

fn write_logins(logins: &BTreeMap<String, StoredCredentials>) -> Result<(), String> {
    let path = auth_file();
    let text = serde_json::to_string_pretty(logins).expect("logins serialize");
    std::fs::write(&path, text).map_err(|err| format!("{}: {}", path.display(), err))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("{}: {}", path.display(), err))?;
    }
    Ok(())
}

/// The login of one MCP server in `~/.minerve/mcp_auth.json`, read and written by rmcp.
pub struct FileCredentialStore {
    server_url: String,
}

impl FileCredentialStore {
    pub fn new(server_url: &str) -> Self {
        Self { server_url: server_url.to_string() }
    }
}

#[async_trait::async_trait]
impl CredentialStore for FileCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let mut logins = read_logins().map_err(AuthError::CredentialStoreError)?;
        Ok(logins.remove(&self.server_url))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let mut logins = read_logins().map_err(AuthError::CredentialStoreError)?;
        logins.insert(self.server_url.clone(), credentials);
        write_logins(&logins).map_err(AuthError::CredentialStoreError)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        forget_login(&self.server_url).map_err(AuthError::CredentialStoreError)
    }
}

/// Forgets the login of a server, e.g. when the server is removed from `/mcp`.
pub fn forget_login(server_url: &str) -> Result<(), String> {
    let mut logins = read_logins()?;
    if logins.remove(server_url).is_some() {
        write_logins(&logins)?;
    }
    Ok(())
}

fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    // The URL is also shown in `/mcp`, so a missing opener only costs a copy-paste.
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Path and query of the request line `GET /callback?code=..&state=.. HTTP/1.1`.
/// None for requests to other paths, e.g. the browser asking for a favicon.
fn callback_target(request_head: &str) -> Option<&str> {
    let target = request_head.lines().next()?.split_whitespace().nth(1)?;
    let path = target.split('?').next()?;
    (path == CALLBACK_PATH).then_some(target)
}

/// Answers browser requests on `listener` until the one to the callback path arrives,
/// and returns its full URL.
async fn wait_for_callback(listener: &TcpListener, redirect_origin: &str) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(|err| err.to_string())?;
        let mut buffer = vec![0u8; 8192];
        let read = stream.read(&mut buffer).await.map_err(|err| err.to_string())?;
        let request_head = String::from_utf8_lossy(&buffer[..read]).to_string();
        let Some(target) = callback_target(&request_head) else {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue;
        };
        let body = "<!doctype html><title>minerve</title>\
                    <p style=\"font-family:sans-serif\">Back to minerve: you can close this tab.</p>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes()).await;
        return Ok(format!("{}{}", redirect_origin, target));
    }
}

/// Runs the browser login for the MCP server at `server_url`; rmcp stores the token through
/// `FileCredentialStore`. `challenge` is the `WWW-Authenticate` header of the server's 401.
/// `on_authorize_url` gets the URL the browser is sent to, so it can be shown in case no browser opens.
pub async fn log_in(server_url: &str, challenge: &str, on_authorize_url: impl FnOnce(String)) -> Result<(), String> {
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|err| err.to_string())?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    let redirect_origin = format!("http://127.0.0.1:{}", port);
    let redirect_uri = format!("{}{}", redirect_origin, CALLBACK_PATH);

    let mut manager = AuthorizationManager::new(server_url).await.map_err(|err| err.to_string())?;
    manager.set_credential_store(FileCredentialStore::new(server_url));
    let resolution = manager
        .resolve_metadata_from_challenge(Some(challenge))
        .await
        .map_err(|err| err.to_string())?;
    manager.set_metadata(resolution.metadata);
    let request = AuthorizationRequest::new(redirect_uri)
        .with_client_name(CLIENT_NAME)
        .with_challenge(challenge);
    let session = AuthorizationSession::new(manager, request)
        .await
        .map_err(|(_, err)| err.to_string())?;

    let authorize_url = session.get_authorization_url().to_string();
    open_in_browser(&authorize_url);
    on_authorize_url(authorize_url);

    let callback_url = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_callback(&listener, &redirect_origin))
        .await
        .map_err(|_| format!("no browser login within {} minutes", LOGIN_TIMEOUT.as_secs() / 60))??;
    session
        .handle_callback_url(&callback_url)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_callback_request() {
        assert_eq!(
            callback_target("GET /callback?code=abc&state=s%201 HTTP/1.1\r\nHost: x\r\n\r\n"),
            Some("/callback?code=abc&state=s%201")
        );
        assert_eq!(callback_target("GET /favicon.ico HTTP/1.1\r\n\r\n"), None);
    }
}
