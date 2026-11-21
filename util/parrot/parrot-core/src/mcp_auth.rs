// SPDX-License-Identifier: Apache-2.0

//! Keycloak login for the MCP Gateway: a real Authorization Code + PKCE
//! browser flow, entirely separate from [`crate::auth`]'s OpenShell-gateway
//! OIDC wiring (different audience, different token store, no stored
//! refresh token to rotate yet since nothing has logged in before).
//!
//! The MCP Gateway itself does not reliably serve the MCP-spec's own OAuth
//! discovery metadata (`.well-known/oauth-protected-resource`) yet, and
//! Keycloak refuses anonymous Dynamic Client Registration — both of which
//! is why `demos/keycloak-oidc/mcp-servers/mcp-inspector.sh` falls back to
//! a pre-fetched password-grant token instead of a real redirect. This
//! module sidesteps both problems the way `openshell gateway login`
//! already does for the OpenShell gateway: skip MCP's own auth-discovery
//! dance entirely and talk to Keycloak directly as a *known* client
//! (`openshell-cli`, already a public client in the demo's realm with
//! `standardFlowEnabled: true` and `http://127.0.0.1/*` /
//! `http://localhost/*` redirect URIs — see `keycloak/realm-export.json`).
//! Keycloak's own OIDC discovery (`/realms/<realm>/.well-known/
//! openid-configuration`) is standard and unaffected by the gateway's gap.

use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::error::{ParrotError, Result};

/// Which Keycloak realm/client to authenticate against. `openshell-cli` is
/// the right default — it's the same client `openshell gateway login`
/// itself uses, already provisioned for a loopback-redirect desktop flow.
#[derive(Clone, Debug)]
pub struct McpAuthConfig {
    /// e.g. `https://keycloak.apps.example.com/realms/openshell`.
    pub issuer: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    /// Skip TLS verification against the issuer — for self-signed dev
    /// clusters only, mirrors `openshell_sdk::oidc::discover`'s own flag.
    pub insecure: bool,
}

/// An access token for the MCP Gateway. Never sent to any frontend's
/// webview/UI layer — only used server-side to set `auth_header` on
/// [`crate::mcp_client::McpGatewayClient`].
#[derive(Clone)]
pub struct McpToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

/// A login in progress: the URL to send the user to, plus everything
/// needed to catch the redirect and finish the exchange. Split from a
/// single `login()` call so the frontend decides how to open the system
/// browser (parrot-core stays UI-agnostic, same as every other module
/// here) — call [`start_login`], open `authorize_url` however fits the
/// frontend, then await [`PendingLogin::complete`].
pub struct PendingLogin {
    pub authorize_url: String,
    listener: TcpListener,
    pkce_verifier: PkceCodeVerifier,
    csrf_token: CsrfToken,
    token_endpoint: String,
    client_id: String,
    redirect_uri: String,
    insecure: bool,
}

/// Starts a login: binds an ephemeral loopback listener for the redirect,
/// discovers Keycloak's endpoints, and builds the authorization URL.
pub async fn start_login(config: &McpAuthConfig) -> Result<PendingLogin> {
    let discovery = openshell_sdk::oidc::discover(&config.issuer, config.insecure)
        .await
        .map_err(ParrotError::Sdk)?;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    // Matches realm-export.json's `http://127.0.0.1/*` redirect URI for
    // openshell-cli — any port and path under that host is pre-registered.
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let client = BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_auth_uri(
            AuthUrl::new(discovery.authorization_endpoint.clone())
                .map_err(|e| ParrotError::Mcp(format!("invalid authorization_endpoint: {e}")))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri.clone())
                .map_err(|e| ParrotError::Mcp(format!("invalid redirect_uri: {e}")))?,
        );

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge)
        // Without this, Keycloak silently reuses an existing SSO session
        // cookie in the system browser instead of showing the login form
        // — confirmed live: logging in as alice, then later clicking
        // Connect as bob in the same browser profile, re-authenticated as
        // alice with no prompt at all. `prompt=login` is the standard
        // OIDC parameter forcing re-authentication regardless of any
        // existing session, which matters a lot for a multi-persona demo.
        .add_extra_param("prompt", "login");
    for scope in &config.scopes {
        request = request.add_scope(Scope::new(scope.clone()));
    }
    let (authorize_url, csrf_token) = request.url();

    Ok(PendingLogin {
        authorize_url: authorize_url.to_string(),
        listener,
        pkce_verifier,
        csrf_token,
        token_endpoint: discovery.token_endpoint,
        client_id: config.client_id.clone(),
        redirect_uri,
        insecure: config.insecure,
    })
}

impl PendingLogin {
    /// Waits for exactly one redirect on the loopback listener, verifies
    /// `state` against CSRF, and exchanges the code for tokens. The
    /// browser tab gets a plain static "you can close this" response —
    /// nothing here talks back to it beyond that.
    pub async fn complete(self) -> Result<McpToken> {
        let (code, state) = accept_one_redirect(&self.listener).await?;
        if state != *self.csrf_token.secret() {
            return Err(ParrotError::Mcp(
                "OAuth state mismatch on redirect (possible CSRF)".to_string(),
            ));
        }

        let client = BasicClient::new(ClientId::new(self.client_id))
            .set_token_uri(
                TokenUrl::new(self.token_endpoint)
                    .map_err(|e| ParrotError::Mcp(format!("invalid token_endpoint: {e}")))?,
            )
            .set_redirect_uri(
                RedirectUrl::new(self.redirect_uri)
                    .map_err(|e| ParrotError::Mcp(format!("invalid redirect_uri: {e}")))?,
            );

        // Built from oauth2's own re-exported `reqwest` rather than
        // openshell_sdk::oidc::http_client's — `AsyncHttpClient` is only
        // implemented for oauth2's pinned reqwest type, which Cargo may or
        // may not unify with openshell-sdk's own reqwest dependency.
        let http_client = oauth2::reqwest::ClientBuilder::new()
            .danger_accept_invalid_certs(self.insecure)
            // Following redirects on a token-endpoint POST opens the
            // client up to SSRF; oauth2's own docs recommend disabling it.
            .redirect(oauth2::reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ParrotError::Mcp(format!("failed to build HTTP client: {e}")))?;
        let token = client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(self.pkce_verifier)
            .request_async(&http_client)
            .await
            .map_err(|e| ParrotError::Mcp(format!("token exchange failed: {e}")))?;

        Ok(McpToken {
            access_token: token.access_token().secret().clone(),
            refresh_token: token.refresh_token().map(|t| t.secret().clone()),
        })
    }
}

/// Accepts one raw HTTP/1.1 request on `listener` and pulls `code`/`state`
/// out of its query string — a full HTTP server (axum, hyper) would be
/// overkill for the single GET a browser's OAuth redirect sends.
async fn accept_one_redirect(listener: &TcpListener) -> Result<(String, String)> {
    let (mut stream, _) = listener.accept().await?;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || n < chunk.len() {
            break;
        }
    }

    let request = String::from_utf8_lossy(&buf);
    let request_line = request.lines().next().unwrap_or("");
    let path_and_query = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = path_and_query.split_once('?').map_or("", |(_, q)| q);
    let params: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();

    let body = "<html><body>Signed in \u{2014} you can close this tab.</body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;

    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map_or(error.as_str(), |d| d.as_str());
        return Err(ParrotError::Mcp(format!(
            "Keycloak returned an error: {description}"
        )));
    }

    let code = params
        .get("code")
        .cloned()
        .ok_or_else(|| ParrotError::Mcp("OAuth redirect had no 'code' parameter".to_string()))?;
    let state = params.get("state").cloned().unwrap_or_default();
    Ok((code, state))
}
