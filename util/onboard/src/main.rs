use std::io::{BufRead, BufReader, Write as _};
use std::net::TcpListener;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use clap::Parser;
use url::Url;

#[derive(Parser)]
#[command(name = "onboard", about = "Automate OpenShell Providers v2 user onboarding via Keycloak")]
struct Cli {
    /// User identifier (e.g. "user2"); provider will be named user-<id>
    #[arg(short = 'u', long)]
    user_id: String,

    /// Keycloak hostname (e.g. keycloak.apps.cluster.example.com)
    #[arg(long, env = "KEYCLOAK_HOST")]
    keycloak_host: String,

    /// Keycloak realm
    #[arg(long, env = "KEYCLOAK_REALM", default_value = "openshell")]
    realm: String,

    /// Public Keycloak client for the browser login
    #[arg(long, env = "KEYCLOAK_CLIENT_ID_CLI", default_value = "openshell-cli")]
    client_id: String,

    /// Confidential client used for refresh material (not needed for public-client flow)
    #[arg(long, env = "KEYCLOAK_CLIENT_ID_GATEWAY", default_value = "openshell-gateway")]
    gateway_client_id: Option<String>,

    /// Confidential client secret (not needed for public-client flow)
    #[arg(long, env = "KEYCLOAK_CLIENT_SECRET")]
    gateway_client_secret: Option<String>,

    /// Path to the provider profile YAML (e.g. demos/keycloak-oidc/providers/user-refresh-profile.yaml)
    #[arg(long)]
    profile: String,

    /// Local port for the OAuth callback listener
    #[arg(long, default_value_t = 9999)]
    port: u16,

    /// Seconds to wait for the callback
    #[arg(long, default_value_t = 120)]
    timeout: u64,

    /// Stop after obtaining the refresh token; print it to stdout
    #[arg(long)]
    token_only: bool,

    /// Skip browser-open attempt; just print the URL
    #[arg(long)]
    no_browser: bool,

    /// Print OpenShell CLI commands without executing them
    #[arg(long)]
    dry_run: bool,

    /// Require valid TLS certificates (disables danger_accept_invalid_certs)
    #[arg(long)]
    strict_tls: bool,
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    refresh_token: Option<String>,
    #[serde(default)]
    refresh_expires_in: u64,
}

fn log(msg: &str) {
    eprintln!("[onboard] {msg}");
}

fn build_http_client(strict_tls: bool) -> reqwest::blocking::Client {
    let accept_invalid = !strict_tls;
    if accept_invalid {
        eprintln!("[onboard] WARNING: TLS certificate verification is disabled. Use --strict-tls for production.");
    }
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(accept_invalid)
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client")
}

fn build_auth_url(host: &str, realm: &str, client_id: &str, port: u16) -> String {
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    format!(
        "https://{host}/realms/{realm}/protocol/openid-connect/auth\
         ?client_id={client_id}\
         &response_type=code\
         &scope=openid%20offline_access\
         &redirect_uri={redirect}",
        redirect = urlencoding::encode(&redirect_uri),
    )
}

fn open_browser(url: &str) -> bool {
    open::that(url).is_ok()
}

fn success_html(keycloak_host: &str, realm: &str) -> String {
    let logout_url = format!(
        "https://{keycloak_host}/realms/{realm}/protocol/openid-connect/logout"
    );
    format!(
        r#"<!DOCTYPE html>
<html><head><title>Onboard</title></head>
<body style="font-family:sans-serif;text-align:center;margin-top:4em">
<h2>Authorization code received</h2>
<p>You can close this tab, or <a href="{logout_url}">log out of Keycloak</a>
first so the next user gets a fresh login prompt.</p>
</body></html>"#
    )
}

fn error_html(msg: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html><head><title>Onboard — Error</title></head>
<body style="font-family:sans-serif;text-align:center;margin-top:4em">
<h2>Authorization failed</h2>
<p>{msg}</p>
</body></html>"#
    )
}

fn http_response(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len(),
    )
}

fn wait_for_callback(port: u16, timeout_secs: u64, keycloak_host: &str, realm: &str) -> Result<String, String> {
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
        .map_err(|e| format!("failed to bind 127.0.0.1:{port}: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        if Instant::now() > deadline {
            return Err(format!("no callback received within {timeout_secs}s"));
        }

        let stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(format!("accept: {e}")),
        };

        stream
            .set_nonblocking(false)
            .map_err(|e| format!("set_blocking: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .ok();

        let mut reader = BufReader::new(&stream);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            continue;
        }

        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_string();

        if !path.starts_with("/callback") {
            let resp = http_response(404, "Not found");
            let _ = (&stream).write_all(resp.as_bytes());
            continue;
        }

        let fake_base = format!("http://127.0.0.1:{port}");
        let full_url = format!("{fake_base}{path}");
        let parsed = Url::parse(&full_url).map_err(|e| format!("bad callback URL: {e}"))?;

        if let Some(err) = parsed.query_pairs().find(|(k, _)| k == "error") {
            let desc = parsed
                .query_pairs()
                .find(|(k, _)| k == "error_description")
                .map(|(_, v)| v.to_string())
                .unwrap_or_default();
            let msg = format!("{}: {desc}", err.1);
            let body = error_html(&msg);
            let resp = http_response(400, &body);
            let _ = (&stream).write_all(resp.as_bytes());
            return Err(msg);
        }

        if let Some(code) = parsed.query_pairs().find(|(k, _)| k == "code") {
            let body = success_html(keycloak_host, realm);
            let resp = http_response(200, &body);
            let _ = (&stream).write_all(resp.as_bytes());
            return Ok(code.1.to_string());
        }

        let body = error_html("No authorization code in callback");
        let resp = http_response(400, &body);
        let _ = (&stream).write_all(resp.as_bytes());
        return Err("no code parameter in callback".into());
    }
}

fn exchange_code(
    client: &reqwest::blocking::Client,
    host: &str,
    realm: &str,
    client_id: &str,
    code: &str,
    port: u16,
) -> Result<TokenResponse, String> {
    let token_url = format!("https://{host}/realms/{realm}/protocol/openid-connect/token");
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let resp = client
        .post(&token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("redirect_uri", &redirect_uri),
        ])
        .send()
        .map_err(|e| format!("token request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("token endpoint returned {status}: {body}"));
    }

    resp.json::<TokenResponse>()
        .map_err(|e| format!("failed to parse token response: {e}"))
}

enum CmdResult {
    Ok,
    AlreadyExists,
}

fn run_cmd(label: &str, program: &str, args: &[&str], dry_run: bool) -> Result<CmdResult, String> {
    let cmd_str = format!("{program} {}", args.join(" "));
    if dry_run {
        eprintln!("[onboard] DRY-RUN: {cmd_str}");
        return Ok(CmdResult::Ok);
    }

    log(&format!("{label}: {cmd_str}"));
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run `{program}`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stdout}{stderr}");
        let lower = combined.to_lowercase();
        if lower.contains("already exists") || lower.contains("already imported") {
            return Ok(CmdResult::AlreadyExists);
        }
        return Err(format!("`{cmd_str}` failed (exit {}): {combined}", output.status));
    }
    Ok(CmdResult::Ok)
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    let provider_name = format!("user-{}", cli.user_id);

    // Step 1: build auth URL
    log("Building authorization URL...");
    let auth_url = build_auth_url(&cli.keycloak_host, &cli.realm, &cli.client_id, cli.port);

    // Step 2: open browser or print URL
    if cli.no_browser {
        log("--no-browser: open this URL manually:");
        eprintln!("{auth_url}");
    } else {
        log("Opening browser...");
        if !open_browser(&auth_url) {
            log("Could not open browser. Open this URL manually:");
            eprintln!("{auth_url}");
        }
    }

    // Step 3: wait for callback
    log(&format!(
        "Waiting for callback on 127.0.0.1:{}  (timeout {}s)...",
        cli.port, cli.timeout
    ));
    let code = wait_for_callback(cli.port, cli.timeout, &cli.keycloak_host, &cli.realm)?;
    log("Authorization code received.");

    // Step 4: exchange code for tokens
    log("Exchanging authorization code for tokens...");
    let http = build_http_client(cli.strict_tls);
    let token_resp = exchange_code(
        &http,
        &cli.keycloak_host,
        &cli.realm,
        &cli.client_id,
        &code,
        cli.port,
    )?;

    let refresh_token = token_resp
        .refresh_token
        .ok_or("token response did not contain a refresh_token")?;

    if token_resp.refresh_expires_in != 0 {
        eprintln!(
            "[onboard] WARNING: refresh_expires_in={} — this is NOT an offline token. \
             The user may need to re-authenticate when it expires.",
            token_resp.refresh_expires_in
        );
    } else {
        log("Confirmed: offline token (refresh_expires_in=0).");
    }

    if cli.token_only {
        println!("{refresh_token}");
        return Ok(());
    }

    // Step 5: call OpenShell CLI
    log("Importing provider profile...");
    match run_cmd(
        "profile import",
        "openshell",
        &["provider", "profile", "import", "-f", &cli.profile],
        cli.dry_run,
    )? {
        CmdResult::AlreadyExists => log("Profile already imported, skipping."),
        CmdResult::Ok => {}
    }

    log(&format!("Creating provider '{provider_name}'..."));
    match run_cmd(
        "provider create",
        "openshell",
        &[
            "provider",
            "create",
            "--name",
            &provider_name,
            "--type",
            "user-scoped-api",
            "--credential",
            "USER_ACCESS_TOKEN=pending",
        ],
        cli.dry_run,
    )? {
        CmdResult::AlreadyExists => log(&format!("Provider '{provider_name}' already exists, skipping creation.")),
        CmdResult::Ok => {}
    }

    log("Configuring refresh strategy...");
    let material_client_id = format!("client_id={}", cli.client_id);
    let material_refresh = format!("refresh_token={refresh_token}");
    run_cmd(
        "refresh configure",
        "openshell",
        &[
            "provider",
            "refresh",
            "configure",
            &provider_name,
            "--credential-key",
            "USER_ACCESS_TOKEN",
            "--strategy",
            "oauth2-refresh-token",
            "--material",
            &material_client_id,
            "--material",
            &material_refresh,
            "--secret-material-key",
            "refresh_token",
        ],
        cli.dry_run,
    )?;

    log("Rotating credential...");
    run_cmd(
        "refresh rotate",
        "openshell",
        &[
            "provider",
            "refresh",
            "rotate",
            &provider_name,
            "--credential-key",
            "USER_ACCESS_TOKEN",
        ],
        cli.dry_run,
    )?;

    log(&format!("User '{provider_name}' onboarded successfully."));
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[onboard] ERROR: {e}");
            ExitCode::FAILURE
        }
    }
}

mod urlencoding {
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
