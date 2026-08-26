use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    Router,
    extract::{Form, Query, State},
    http::HeaderMap,
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use clap::Parser;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::process::Command;

const SESSION_COOKIE: &str = "onboarding_session";

/// Self-service OpenShell onboarding web app — Option B (see
/// demos/keycloak-oidc/docs/self-service-onboarding.md): admin
/// pre-provisions the workspace, provider (with `pending` credential
/// material), sandbox, MCP config, and agent harness out of band. This
/// service's entire job is the *last mile*: let a user log in via Keycloak
/// as themselves, then run `provider refresh configure`/`refresh rotate`
/// against the provider(s) admin already created for them. It never runs
/// `workspace create`, `provider create`, `provider profile import`, or
/// `sandbox create`.
#[derive(Parser, Clone)]
#[command(about = "Self-service OpenShell onboarding web app (Option B: token-attach only)")]
struct Config {
    /// Keycloak hostname (e.g. keycloak.apps.cluster.example.com)
    #[arg(long, env = "KEYCLOAK_HOST")]
    keycloak_host: String,

    /// Keycloak realm
    #[arg(long, env = "KEYCLOAK_REALM", default_value = "openshell")]
    keycloak_realm: String,

    /// Public client (PKCE-secured) used for THIS app's own user-facing
    /// login — NOT openshell-cli. Must be public, not confidential:
    /// confirmed live that Providers v2's refresh grant (run by the
    /// gateway itself, using only the `client_id` material configured
    /// below — no secret) gets a 401 from Keycloak if the client is
    /// confidential, matching `util/onboard/PROMPT.md`'s own warning that
    /// this refresh mechanism requires a public client. Keycloak refresh
    /// tokens are also client-bound, so this exact client_id is what gets
    /// passed as `--material client_id=` to `provider refresh configure`
    /// below; using openshell-cli there instead (copy-pasting
    /// `03-onboard-user.sh`'s pattern) would make Keycloak reject the
    /// refresh grant for a different reason (wrong client entirely).
    #[arg(
        long,
        env = "ONBOARDING_WEB_CLIENT_ID",
        default_value = "openshell-onboarding-web"
    )]
    client_id: String,

    /// This client's secret, if it has one. Normally unset — the client is
    /// public, secured by PKCE on the authorization code exchange instead
    /// (see `client_id` above for why it can't be confidential).
    #[arg(long, env = "ONBOARDING_WEB_CLIENT_SECRET")]
    client_secret: Option<String>,

    /// This app's own externally-reachable base URL (e.g.
    /// https://onboarding-web-<ns>.<apps-domain>), no trailing slash — used
    /// to build the exact `redirect_uri` registered on the Keycloak client.
    #[arg(long, env = "ONBOARDING_WEB_BASE_URL")]
    base_url: String,

    /// Address to bind the HTTP server to.
    #[arg(long, env = "HOST", default_value = "0.0.0.0")]
    host: String,

    #[arg(long, env = "PORT", default_value_t = 8080)]
    port: u16,

    /// XDG_CONFIG_HOME for the backend's own standing Platform-Admin
    /// `openshell` session (see self-service-onboarding.md, decision #2).
    /// If unset, `openshell` invocations inherit this process's own
    /// environment unchanged — useful for running locally against
    /// whatever gateway session is already active in this shell.
    #[arg(long, env = "OPENSHELL_ADMIN_XDG_CONFIG_HOME")]
    admin_xdg_config_home: Option<String>,

    #[arg(long, env = "OPENSHELL_ADMIN_XDG_STATE_HOME")]
    admin_xdg_state_home: Option<String>,

    /// Require valid TLS certificates when talking to Keycloak (default:
    /// accept self-signed — lab clusters commonly use them). Mirrors
    /// util/onboard's --strict-tls.
    #[arg(long, env = "STRICT_TLS")]
    strict_tls: bool,

    /// How long a pending login (state/PKCE) or pending attach (post-login,
    /// pre-selection) session stays valid, in seconds.
    #[arg(long, env = "SESSION_TTL_SECS", default_value_t = 600)]
    session_ttl_secs: u64,
}

enum SessionData {
    PendingLogin {
        oauth_state: String,
        pkce_verifier: String,
    },
    PendingAttach {
        username: String,
        refresh_token: String,
    },
}

struct SessionEntry {
    data: SessionData,
    created_at: Instant,
}

type Sessions = Arc<Mutex<HashMap<String, SessionEntry>>>;

struct AppState {
    config: Config,
    http: reqwest::Client,
    sessions: Sessions,
}

fn is_expired(entry: &SessionEntry, ttl_secs: u64) -> bool {
    entry.created_at.elapsed().as_secs() >= ttl_secs
}

fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

// Timestamps are deliberately omitted — the cluster's log collector stamps
// ingestion time on every stdout line, which is what the audit trail this
// service needs actually relies on.
fn log_event(action: &str, subject: &str, provider: Option<&str>, outcome: &str, detail: &str) {
    let line = serde_json::json!({
        "action": action,
        "subject": subject,
        "provider": provider,
        "outcome": outcome,
        "detail": detail,
    });
    println!("{line}");
}

// --- PKCE / OAuth helpers ---

fn generate_pkce_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn generate_opaque_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    #[serde(default)]
    refresh_expires_in: u64,
}

async fn exchange_code(
    http: &reqwest::Client,
    config: &Config,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, String> {
    let token_url = format!(
        "https://{}/realms/{}/protocol/openid-connect/token",
        config.keycloak_host, config.keycloak_realm
    );

    let mut params = vec![
        ("grant_type", "authorization_code"),
        ("client_id", config.client_id.as_str()),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier),
    ];
    if let Some(secret) = &config.client_secret {
        params.push(("client_secret", secret.as_str()));
    }

    let resp = http
        .post(&token_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token endpoint returned {status}: {body}"));
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| format!("failed to parse token response: {e}"))
}

// A token we obtained ourselves, directly from Keycloak's token endpoint
// over a TLS connection this service authenticated (client_id +
// client_secret) — no attacker-supplied token reaches this path, unlike a
// caller-presented bearer token an MCP server would have to validate. So a
// raw decode without signature verification is safe here.
fn decode_jwt_claim(token: &str, claim: &str) -> Option<String> {
    let payload_b64 = token.split('.').nth(1)?;
    let payload = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    value.get(claim)?.as_str().map(|s| s.to_string())
}

// --- openshell CLI wrapper ---

async fn run_openshell(config: &Config, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("openshell");
    cmd.args(args);
    if let Some(v) = &config.admin_xdg_config_home {
        cmd.env("XDG_CONFIG_HOME", v);
    }
    if let Some(v) = &config.admin_xdg_state_home {
        cmd.env("XDG_STATE_HOME", v);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("failed to run openshell: {e}"))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "openshell {} failed (exit {}): {stdout}{stderr}",
            args.join(" "),
            output.status
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[derive(Deserialize)]
struct ProviderListEntry {
    name: String,
}

// Confirmed live: `openshell provider list --workspace <ws> --output json`
// returns a JSON array of objects with a `name` field, matching
// `ProviderListEntry` exactly. The plain-text fallback below is kept for
// resilience against a future CLI version dropping `--output json`, but
// isn't expected to be exercised in normal operation.
async fn list_providers(config: &Config, workspace: &str) -> Result<Vec<String>, String> {
    match run_openshell(
        config,
        &[
            "provider",
            "list",
            "--workspace",
            workspace,
            "--output",
            "json",
        ],
    )
    .await
    {
        Ok(stdout) => {
            if let Ok(entries) = serde_json::from_str::<Vec<ProviderListEntry>>(&stdout) {
                return Ok(entries.into_iter().map(|e| e.name).collect());
            }
            Ok(parse_provider_list_text(&stdout))
        }
        Err(_) => {
            let stdout =
                run_openshell(config, &["provider", "list", "--workspace", workspace]).await?;
            Ok(parse_provider_list_text(&stdout))
        }
    }
}

fn parse_provider_list_text(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .skip(1) // assumed header row
        .filter_map(|line| line.split_whitespace().next())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

async fn activate_provider(
    config: &Config,
    workspace: &str,
    provider: &str,
    refresh_token: &str,
) -> Result<(), String> {
    let material_client_id = format!("client_id={}", config.client_id);
    let material_refresh_token = format!("refresh_token={refresh_token}");

    run_openshell(
        config,
        &[
            "provider",
            "refresh",
            "configure",
            provider,
            "--credential-key",
            "USER_ACCESS_TOKEN",
            "--strategy",
            "oauth2-refresh-token",
            "--material",
            &material_client_id,
            "--material",
            &material_refresh_token,
            "--secret-material-key",
            "refresh_token",
            "--workspace",
            workspace,
        ],
    )
    .await?;

    run_openshell(
        config,
        &[
            "provider",
            "refresh",
            "rotate",
            provider,
            "--credential-key",
            "USER_ACCESS_TOKEN",
            "--workspace",
            workspace,
        ],
    )
    .await?;

    Ok(())
}

// --- HTML rendering (hand-rolled, same lightweight style as
// util/onboard's success_html/error_html — no templating engine, no JS) ---

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html><head><title>{title}</title></head>
<body style="font-family:sans-serif;max-width:640px;margin:4em auto;line-height:1.5">
<h2>{title}</h2>
{body}
</body></html>"#
    )
}

fn index_page() -> String {
    page(
        "Onboard to OpenShell",
        r#"<p>Sign in with your Keycloak account to activate your OpenShell credential.</p>
<p><a href="/login" style="display:inline-block;padding:0.6em 1.2em;background:#043;color:#fff;text-decoration:none;border-radius:4px">Sign in with Keycloak</a></p>"#,
    )
}

fn error_page(msg: &str) -> String {
    page(
        "Onboarding error",
        &format!(
            "<p style=\"color:#a00\">{}</p><p><a href=\"/\">Start over</a></p>",
            html_escape(msg)
        ),
    )
}

fn select_page_html(username: &str, providers: &[String]) -> String {
    let checkboxes: String = providers
        .iter()
        .map(|p| {
            let name = html_escape(p);
            format!(
                r#"<label style="display:block;margin:0.5em 0"><input type="checkbox" name="activate_{name}" value="1" checked> {name}</label>"#
            )
        })
        .collect();

    let note = if providers.len() == 1 {
        "<p>Your admin has provisioned one provider for you — it's pre-selected below. \
         This activates every sandbox that uses it, now and in the future; you won't \
         need to do this again for additional sandboxes.</p>"
    } else {
        "<p>Your admin has provisioned more than one provider for you. Choose which to \
         activate.</p>"
    };

    page(
        "Activate your OpenShell credential",
        &format!(
            r#"<p>Signed in as <strong>{user}</strong>.</p>
{note}
<form method="post" action="/attach">
{boxes}
<button type="submit" style="margin-top:1em;padding:0.5em 1em">Activate</button>
</form>"#,
            user = html_escape(username),
            boxes = checkboxes,
        ),
    )
}

fn result_page_html(username: &str, results: &[(String, Result<(), String>)]) -> String {
    let rows: String = results
        .iter()
        .map(|(name, outcome)| match outcome {
            Ok(()) => format!("<li>activated: {}</li>", html_escape(name)),
            Err(e) => format!(
                "<li>failed: {} — {}</li>",
                html_escape(name),
                html_escape(e)
            ),
        })
        .collect();

    page(
        "Onboarding complete",
        &format!(
            r#"<p>Results for <strong>{user}</strong>:</p><ul>{rows}</ul>
<p>You can close this tab and connect to your sandbox as usual.</p>"#,
            user = html_escape(username),
        ),
    )
}

// --- Handlers ---

async fn index() -> Html<String> {
    Html(index_page())
}

async fn login(State(state): State<Arc<AppState>>, jar: CookieJar) -> impl IntoResponse {
    let session_id = generate_opaque_id();
    let oauth_state = generate_opaque_id();
    let pkce_verifier = generate_pkce_verifier();
    let pkce_challenge_value = pkce_challenge(&pkce_verifier);

    {
        let mut sessions = state.sessions.lock().unwrap();
        sessions.insert(
            session_id.clone(),
            SessionEntry {
                data: SessionData::PendingLogin {
                    oauth_state: oauth_state.clone(),
                    pkce_verifier,
                },
                created_at: Instant::now(),
            },
        );
    }

    let redirect_uri = format!("{}/callback", state.config.base_url);
    let auth_url = format!(
        "https://{host}/realms/{realm}/protocol/openid-connect/auth\
         ?client_id={client_id}\
         &response_type=code\
         &scope=openid%20offline_access\
         &redirect_uri={redirect_uri}\
         &state={oauth_state}\
         &code_challenge={challenge}\
         &code_challenge_method=S256",
        host = state.config.keycloak_host,
        realm = state.config.keycloak_realm,
        client_id = urlencoding::encode(&state.config.client_id),
        redirect_uri = urlencoding::encode(&redirect_uri),
        oauth_state = urlencoding::encode(&oauth_state),
        challenge = urlencoding::encode(&pkce_challenge_value),
    );

    let cookie = Cookie::build((SESSION_COOKIE, session_id))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path("/")
        .build();

    (jar.add(cookie), Redirect::to(&auth_url))
}

#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    let Some(session_id) = jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) else {
        return Html(error_page("No session cookie found — please start over.")).into_response();
    };

    if let Some(err) = params.error {
        let desc = params.error_description.unwrap_or_default();
        log_event(
            "login",
            "unknown",
            None,
            "failure",
            &format!("{err}: {desc}"),
        );
        return Html(error_page(&format!(
            "Keycloak returned an error: {err}: {desc}"
        )))
        .into_response();
    }

    let (Some(code), Some(returned_state)) = (params.code, params.state) else {
        return Html(error_page(
            "Missing authorization code or state in callback.",
        ))
        .into_response();
    };

    let pkce_verifier = {
        let mut sessions = state.sessions.lock().unwrap();
        match sessions.remove(&session_id) {
            Some(entry) if !is_expired(&entry, state.config.session_ttl_secs) => match entry.data {
                SessionData::PendingLogin {
                    oauth_state,
                    pkce_verifier,
                } if oauth_state == returned_state => pkce_verifier,
                _ => {
                    return Html(error_page(
                        "Login state mismatch — please start over (this can happen if you \
                         opened the login link twice, or waited too long).",
                    ))
                    .into_response();
                }
            },
            _ => {
                return Html(error_page(
                    "Login session expired or not found — please start over.",
                ))
                .into_response();
            }
        }
    };

    let redirect_uri = format!("{}/callback", state.config.base_url);
    let token_resp = match exchange_code(
        &state.http,
        &state.config,
        &code,
        &pkce_verifier,
        &redirect_uri,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            log_event("login", "unknown", None, "failure", &e);
            return Html(error_page(&format!("Token exchange failed: {e}"))).into_response();
        }
    };

    let Some(refresh_token) = token_resp.refresh_token else {
        return Html(error_page("Keycloak did not return a refresh token.")).into_response();
    };

    if token_resp.refresh_expires_in != 0 {
        return Html(error_page(&format!(
            "Keycloak issued a token that expires in {}s — this is not an offline token. \
             Contact an admin: the Keycloak client must request the offline_access scope.",
            token_resp.refresh_expires_in
        )))
        .into_response();
    }

    let Some(access_token) = token_resp.access_token else {
        return Html(error_page("Keycloak did not return an access token.")).into_response();
    };

    let Some(username) = decode_jwt_claim(&access_token, "preferred_username") else {
        return Html(error_page("Could not read your username from the token.")).into_response();
    };

    log_event("login", &username, None, "success", &client_ip(&headers));

    {
        let mut sessions = state.sessions.lock().unwrap();
        sessions.insert(
            session_id,
            SessionEntry {
                data: SessionData::PendingAttach {
                    username,
                    refresh_token,
                },
                created_at: Instant::now(),
            },
        );
    }

    Redirect::to("/select").into_response()
}

async fn select_page(State(state): State<Arc<AppState>>, jar: CookieJar) -> impl IntoResponse {
    let Some(session_id) = jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) else {
        return Redirect::to("/").into_response();
    };

    let username = {
        let sessions = state.sessions.lock().unwrap();
        match sessions.get(&session_id) {
            Some(entry) if !is_expired(entry, state.config.session_ttl_secs) => match &entry.data {
                SessionData::PendingAttach { username, .. } => username.clone(),
                _ => return Redirect::to("/").into_response(),
            },
            _ => return Redirect::to("/").into_response(),
        }
    };

    let providers = match list_providers(&state.config, &username).await {
        Ok(p) => p,
        Err(e) => {
            return Html(error_page(&format!(
                "Could not look up your OpenShell workspace ('{username}'): {e}. \
                 Your workspace may not have been provisioned yet — contact an admin."
            )))
            .into_response();
        }
    };

    if providers.is_empty() {
        return Html(error_page(&format!(
            "No OpenShell provider found in workspace '{username}'. Your account hasn't \
             been provisioned yet — contact an admin."
        )))
        .into_response();
    }

    Html(select_page_html(&username, &providers)).into_response()
}

async fn attach(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(session_id) = jar.get(SESSION_COOKIE).map(|c| c.value().to_string()) else {
        return Redirect::to("/").into_response();
    };

    let (username, refresh_token) = {
        let mut sessions = state.sessions.lock().unwrap();
        match sessions.remove(&session_id) {
            Some(entry) if !is_expired(&entry, state.config.session_ttl_secs) => match entry.data {
                SessionData::PendingAttach {
                    username,
                    refresh_token,
                } => (username, refresh_token),
                _ => {
                    return Html(error_page("Session expired — please start over."))
                        .into_response();
                }
            },
            _ => return Html(error_page("Session expired — please start over.")).into_response(),
        }
    };

    let selected: Vec<String> = form
        .iter()
        .filter(|(_, v)| v.as_str() == "1")
        .filter_map(|(k, _)| k.strip_prefix("activate_").map(|s| s.to_string()))
        .collect();

    if selected.is_empty() {
        return Html(error_page("No provider selected.")).into_response();
    }

    let ip = client_ip(&headers);
    let mut results = Vec::new();
    for provider in &selected {
        let outcome = activate_provider(&state.config, &username, provider, &refresh_token).await;
        let (outcome_str, detail) = match &outcome {
            Ok(()) => ("success", String::new()),
            Err(e) => ("failure", e.clone()),
        };
        log_event(
            "attach",
            &username,
            Some(provider),
            outcome_str,
            &format!("{ip} {detail}"),
        );
        results.push((provider.clone(), outcome));
    }

    Html(result_page_html(&username, &results)).into_response()
}

// --- Main ---

#[tokio::main]
async fn main() {
    let config = Config::parse();

    if !config.strict_tls {
        eprintln!(
            "[onboarding-web] WARNING: TLS certificate verification is disabled for the \
             Keycloak client. Use --strict-tls for production."
        );
    }

    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(!config.strict_tls)
        .build()
        .expect("failed to build HTTP client");

    let ttl = config.session_ttl_secs;
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));

    // Bound memory over long uptimes — pending logins/attaches older than
    // the TTL are dropped even if the user never comes back to finish.
    {
        let sessions = sessions.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let mut sessions = sessions.lock().unwrap();
                sessions.retain(|_, entry| !is_expired(entry, ttl));
            }
        });
    }

    let host = config.host.clone();
    let port = config.port;

    let state = Arc::new(AppState {
        config,
        http,
        sessions,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/login", get(login))
        .route("/callback", get(callback))
        .route("/select", get(select_page))
        .route("/attach", post(attach))
        .with_state(state);

    let addr = format!("{host}:{port}");
    eprintln!("[onboarding-web] listening on {addr}");
    let listener = TcpListener::bind(&addr).await.expect("failed to bind");
    axum::serve(listener, app).await.expect("server error");
}

mod urlencoding {
    // Hand-rolled to match util/onboard's own module — avoids taking a
    // dependency on the `url` crate for one function.
    pub fn encode(input: &str) -> String {
        let mut out = String::with_capacity(input.len() * 3);
        for b in input.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char);
                }
                _ => {
                    out.push('%');
                    out.push_str(&format!("{b:02X}"));
                }
            }
        }
        out
    }
}
