// SPDX-License-Identifier: Apache-2.0

//! "Are the files in order?"
//!
//! Every check here corresponds to a failure that has actually cost time
//! while running the keycloak-oidc guide, and each is phrased to name the
//! fix rather than just the symptom. The three that matter most, because
//! none of them look like a file problem when they bite:
//!
//! * **Stale cluster registration.** `metadata.json` still points at the
//!   cluster you ran the demo against last week. The CLI reports
//!   "isn't a member of this workspace", which sends you looking at
//!   Keycloak roles instead of at a file.
//! * **Wrong persona in a persona's tree.** `oc-bob/` holding alice's
//!   token passes every existence check and fails confusingly later.
//! * **A `ca.crt` missing the Route's issuing chain** on the Let's Encrypt
//!   path, which surfaces as `invalid peer certificate: UnknownIssuer`.

use std::path::Path;

use crate::layout::{CANONICAL_IDENTITIES, IdentityPaths, Layout, Scope};
use crate::probe::{self, GatewayMetadata, OidcBundle};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Context, never a problem.
    Info,
    Ok,
    /// The demo will probably still run, but something needs attention.
    Warn,
    /// The guide cannot run in this state.
    Fail,
}

impl Level {
    pub fn marker(self) -> &'static str {
        match self {
            Level::Info => "·",
            Level::Ok => "✓",
            Level::Warn => "!",
            Level::Fail => "✗",
        }
    }

    pub fn paint(self, text: &str) -> String {
        match self {
            Level::Info => crate::fmt::dim(text),
            Level::Ok => crate::fmt::green(text),
            Level::Warn => crate::fmt::yellow(text),
            Level::Fail => crate::fmt::red(text),
        }
    }
}

#[derive(serde::Serialize)]
pub struct Check {
    /// `env`, `shared`, or an identity id.
    pub scope: String,
    pub item: String,
    pub level: Level,
    pub detail: String,
    /// What to do about it. Only set for Warn/Fail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

#[derive(Default, serde::Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    fn push(&mut self, scope: &str, item: &str, level: Level, detail: impl Into<String>) {
        self.checks.push(Check {
            scope: scope.to_string(),
            item: item.to_string(),
            level,
            detail: detail.into(),
            fix: None,
        });
    }

    fn push_fix(
        &mut self,
        scope: &str,
        item: &str,
        level: Level,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) {
        self.checks.push(Check {
            scope: scope.to_string(),
            item: item.to_string(),
            level,
            detail: detail.into(),
            fix: Some(fix.into()),
        });
    }

    pub fn worst(&self) -> Level {
        self.checks
            .iter()
            .map(|c| c.level)
            .max()
            .unwrap_or(Level::Ok)
    }

    pub fn count(&self, level: Level) -> usize {
        self.checks.iter().filter(|c| c.level == level).count()
    }

    /// Scopes in the order they were first checked, so output groups
    /// without needing a second pass over the data.
    pub fn scopes(&self) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for check in &self.checks {
            if !seen.contains(&check.scope) {
                seen.push(check.scope.clone());
            }
        }
        seen
    }
}

/// Run every check for the requested scopes and identities.
///
/// `explicit` says whether the caller named these identities with
/// `-i`, as opposed to them coming from directory discovery. It changes
/// how an identity registered against another cluster is judged — see
/// [`check_identity`].
pub fn run(layout: &Layout, identities: &[String], explicit: bool, scopes: &[Scope]) -> Report {
    let mut report = Report::default();

    // The .env is checked unconditionally: it supplies the expected
    // endpoint that the identity checks compare against, so its absence
    // changes how those read and must be visible first.
    check_env(layout, &mut report);

    if scopes.contains(&Scope::Shared) {
        check_shared(layout, &mut report);
    }
    if scopes.contains(&Scope::Identities) {
        for id in identities {
            check_identity(layout, id, explicit, &mut report);
        }
    }
    if scopes.contains(&Scope::Demo) {
        check_demo_artifacts(layout, &mut report);
    }

    report
}

fn check_env(layout: &Layout, report: &mut Report) {
    let Some(demo_dir) = &layout.demo_dir else {
        report.push_fix(
            "env",
            "demo directory",
            Level::Fail,
            "not found from the current directory",
            "run demo-env from inside the checkout, or pass --demo-dir <path>/demos/keycloak-oidc",
        );
        return;
    };
    report.push(
        "env",
        "demo directory",
        Level::Info,
        crate::fmt::tilde(demo_dir),
    );

    let env_path = layout
        .env
        .path
        .clone()
        .unwrap_or_else(|| demo_dir.join(".env"));
    if !env_path.is_file() {
        report.push_fix(
            "env",
            ".env",
            Level::Fail,
            "missing",
            format!(
                "cp {}/.env.example {} and fill it in",
                crate::fmt::tilde(demo_dir),
                crate::fmt::tilde(&env_path)
            ),
        );
        return;
    }
    report.push("env", ".env", Level::Ok, crate::fmt::tilde(&env_path));

    // Only the variables this tool's own checks depend on. The demo needs
    // many more (API keys, images), but those fail loudly at the step that
    // uses them, whereas these three fail as a confusing cluster mismatch.
    for key in [
        "CLUSTER_APPS_DOMAIN",
        "OPENSHELL_NAMESPACE",
        "KEYCLOAK_HOST",
    ] {
        match layout.env.get(key) {
            Some(value) => report.push("env", key, Level::Ok, value.to_string()),
            None => report.push_fix(
                "env",
                key,
                Level::Fail,
                "unset or empty",
                format!("set {key} in {}", crate::fmt::tilde(&env_path)),
            ),
        }
    }

    // AGENTS.md: a namespace that repeats the prefix pushes the Route
    // FQDN past the 64-byte X.509 CommonName limit on the ACME path.
    if let Some(ns) = layout.env.namespace()
        && ns.starts_with("openshell-")
    {
        report.push_fix(
            "env",
            "namespace prefix",
            Level::Warn,
            format!("'{ns}' starts with 'openshell-', making the Route host redundant"),
            "use a short demo slug, e.g. keycloak-oidc-demo",
        );
    }

    if let Some(host) = layout.env.route_host() {
        let level = if host.len() > 64 && layout.env.letsencrypt_issuer().is_some() {
            Level::Warn
        } else {
            Level::Info
        };
        let detail = if level == Level::Warn {
            format!("{host} ({} bytes, over the 64-byte CN limit)", host.len())
        } else {
            host.clone()
        };
        if level == Level::Warn {
            report.push_fix(
                "env",
                "route host",
                level,
                detail,
                "shorten OPENSHELL_NAMESPACE, or drop LETSENCRYPT_CLUSTER_ISSUER for the self-signed CA",
            );
        } else {
            report.push("env", "route host", level, detail);
        }
    }

    report.push("env", "gateway name", Level::Info, layout.gateway.clone());
}

fn check_shared(layout: &Layout, report: &mut Report) {
    let path = &layout.ca_bundle;
    if !path.is_file() {
        // Not fatal on its own: a publicly-trusted ingress cert needs no
        // bundle at all. It is fatal with the default self-signed ingress
        // cert, which is why this is a warning with the generating step
        // named rather than silence.
        report.push_fix(
            "shared",
            "CA bundle",
            Level::Warn,
            format!("missing: {}", crate::fmt::tilde(path)),
            "needed with a self-signed ingress cert — see the README's \"OIDC issuer TLS trust\" section",
        );
        return;
    }
    match probe::read_pem(path) {
        Ok(info) if info.certificates == 0 => report.push_fix(
            "shared",
            "CA bundle",
            Level::Fail,
            "contains no certificates",
            "regenerate it — see the README's \"OIDC issuer TLS trust\" section",
        ),
        Ok(info) => {
            report.push(
                "shared",
                "CA bundle",
                Level::Ok,
                format!(
                    "{} certificate{}, {}",
                    info.certificates,
                    if info.certificates == 1 { "" } else { "s" },
                    crate::fmt::tilde(path)
                ),
            );
        }
        Err(e) => report.push("shared", "CA bundle", Level::Warn, e),
    }
}

fn check_identity(layout: &Layout, id: &str, explicit: bool, report: &mut Report) {
    let paths = layout.identity(id);
    if !paths.exists() {
        report.push_fix(
            id,
            "identity tree",
            Level::Fail,
            format!("missing: {}", crate::fmt::tilde(&paths.root)),
            format!("./scripts/relogin-identity.sh {id}"),
        );
        return;
    }

    // Parked, not stale. Keeping a second cluster's logins alongside the
    // current ones under a suffixed directory name
    // (`oc-alice-<cluster>/`) is a reasonable thing to do — it is the
    // hand-rolled version of what slots are for. Such a set is registered
    // against another cluster by design, so reporting four stale logins
    // for it is noise that buries the real check.
    //
    // The real check is still the valuable one and is kept intact for the
    // case it exists for: one of the canonical four pointing elsewhere
    // means a run against a second cluster has overwritten this run's
    // logins. So only a *discovered*, non-canonical identity is allowed to
    // be parked — and naming it with `-i` opts back into the full checks.
    if !explicit
        && !CANONICAL_IDENTITIES.contains(&id)
        && let Some(elsewhere) = parked_endpoint(layout, &paths)
    {
        report.push(
            id,
            "parked",
            Level::Info,
            format!(
                "registered against {elsewhere}, not this cluster — skipped (check it with `-i {id}`)"
            ),
        );
        return;
    }

    report.push(
        id,
        "identity tree",
        Level::Info,
        crate::fmt::tilde(&paths.root),
    );

    check_registration(layout, id, &paths, report);
    check_token(id, &paths, report);
    check_mtls(layout, id, &paths, report);
}

/// The other cluster this identity is registered against, if it is
/// readable and genuinely differs from the one `.env` names. `None` when
/// it matches, when either endpoint is unknown, or when the registration
/// can't be read — all cases the normal checks should report on properly
/// rather than silently skip.
fn parked_endpoint(layout: &Layout, paths: &IdentityPaths) -> Option<String> {
    let expected = layout.env.expected_endpoint()?;
    let actual = GatewayMetadata::read(&paths.metadata()).ok()?.endpoint?;
    (actual != expected).then_some(actual)
}

fn check_registration(layout: &Layout, id: &str, paths: &IdentityPaths, report: &mut Report) {
    let path = paths.metadata();
    if !path.is_file() {
        report.push_fix(
            id,
            "registration",
            Level::Fail,
            format!("no gateway '{}' registered", layout.gateway),
            format!("./scripts/relogin-identity.sh {id}"),
        );
        return;
    }
    let metadata = match GatewayMetadata::read(&path) {
        Ok(m) => m,
        Err(e) => {
            report.push_fix(
                id,
                "registration",
                Level::Fail,
                format!("metadata.json unreadable: {e}"),
                format!("./scripts/relogin-identity.sh {id}"),
            );
            return;
        }
    };

    match (&metadata.endpoint, layout.env.expected_endpoint()) {
        (Some(actual), Some(expected)) if *actual == expected => {
            report.push(id, "registration", Level::Ok, actual.clone());
        }
        (Some(actual), Some(expected)) => {
            // The same comparison lib-use-identity.sh makes before it
            // refuses to switch identity.
            report.push_fix(
                id,
                "registration",
                Level::Fail,
                format!("points at a different cluster: {actual} (expected {expected})"),
                format!("./scripts/relogin-identity.sh {id}  # admin first if admin is stale too"),
            );
        }
        (Some(actual), None) => {
            report.push(
                id,
                "registration",
                Level::Warn,
                format!("{actual} (cannot cross-check — .env is incomplete)"),
            );
        }
        (None, _) => {
            report.push(
                id,
                "registration",
                Level::Warn,
                "metadata.json has no gateway endpoint field",
            );
        }
    }

    if let Some(mode) = &metadata.auth_mode
        && mode != "oidc"
    {
        report.push(
            id,
            "auth mode",
            Level::Warn,
            format!("'{mode}', but this demo registers gateways with OIDC"),
        );
    }
}

fn check_token(id: &str, paths: &IdentityPaths, report: &mut Report) {
    let path = paths.oidc_token();
    if !path.is_file() {
        report.push_fix(
            id,
            "OIDC token",
            Level::Fail,
            "no oidc_token.json — this identity has never completed a browser login",
            format!("./scripts/relogin-identity.sh {id}"),
        );
        return;
    }
    let bundle = match OidcBundle::read(&path) {
        Ok(b) => b,
        Err(e) => {
            report.push_fix(
                id,
                "OIDC token",
                Level::Fail,
                format!("oidc_token.json unreadable: {e}"),
                format!("./scripts/relogin-identity.sh {id}"),
            );
            return;
        }
    };

    if !bundle.has_refresh_token {
        // Without `offline_access` there is nothing to rotate, so the
        // session dies at the first access-token expiry mid-demo.
        report.push_fix(
            id,
            "refresh token",
            Level::Fail,
            "absent — the session cannot be renewed",
            format!(
                "./scripts/relogin-identity.sh {id}  # registers with --oidc-scopes \"openid offline_access\""
            ),
        );
    }

    match bundle.expires_at {
        Some(exp) => {
            let remaining = crate::fmt::seconds_until(exp);
            if remaining > 0 {
                report.push(
                    id,
                    "access token",
                    Level::Ok,
                    format!("valid for {}", crate::fmt::human_duration(remaining)),
                );
            } else {
                // Expected and harmless whenever a refresh token is
                // present — the CLI rotates on next use.
                let level = if bundle.has_refresh_token {
                    Level::Info
                } else {
                    Level::Fail
                };
                report.push(
                    id,
                    "access token",
                    level,
                    format!(
                        "expired {} ago{}",
                        crate::fmt::human_duration(remaining),
                        if bundle.has_refresh_token {
                            " (will refresh on next use)"
                        } else {
                            " and there is no refresh token"
                        }
                    ),
                );
            }
        }
        None => report.push(id, "access token", Level::Info, "expiry unknown"),
    }

    match &bundle.subject {
        Some(subject) if subject_matches(id, subject) => {
            report.push(id, "token subject", Level::Ok, subject.clone());
        }
        Some(subject) => report.push_fix(
            id,
            "token subject",
            Level::Fail,
            format!("token belongs to '{subject}', not '{id}'"),
            format!(
                "a login for the wrong persona landed here — log out of Keycloak, then ./scripts/relogin-identity.sh {id}"
            ),
        ),
        None => report.push(id, "token subject", Level::Info, "unknown"),
    }

    if let Some(issuer) = &bundle.issuer {
        let client = bundle.client_id.as_deref().unwrap_or("client ?");
        report.push(id, "issuer", Level::Info, format!("{issuer} ({client})"));
    }
}

/// Does this access token belong to the persona whose directory it is in?
///
/// The directory name is a *local* label and need not equal the Keycloak
/// username. Three ways they legitimately differ:
///
/// * admin's directory is `oc-admin`, its Keycloak account is
///   `openshell-admin` (see the README's step 1a credentials note);
/// * the issuer may return an email, whose local part is the username;
/// * a directory may carry a suffix to park a second cluster's logins
///   beside the current ones — `oc-alice-<cluster>/` still holds a token
///   for plain `alice`, and calling that a wrong-persona login is false.
///
/// What stays caught is the case this check exists for: alice's token
/// landing in `oc-bob/`. The deliberate looseness is that a directory
/// named `<subject>-<anything>` is accepted for `<subject>`, so a Keycloak
/// user literally named `alice` would be accepted in `oc-alice-bob/`.
fn subject_matches(id: &str, subject: &str) -> bool {
    if subject == id {
        return true;
    }
    let subject = subject.split('@').next().unwrap_or(subject);
    if subject == id {
        return true;
    }
    let base = subject.strip_prefix("openshell-").unwrap_or(subject);
    base == id || id.starts_with(&format!("{base}-"))
}

fn check_mtls(layout: &Layout, id: &str, paths: &IdentityPaths, report: &mut Report) {
    let ca = paths.ca_crt();
    let crt = paths.tls_crt();
    let key = paths.tls_key();

    for (label, path) in [
        ("mtls ca.crt", &ca),
        ("mtls tls.crt", &crt),
        ("mtls tls.key", &key),
    ] {
        if !path.is_file() {
            report.push_fix(
                id,
                label,
                Level::Fail,
                "missing",
                format!("./scripts/relogin-identity.sh {id}"),
            );
        } else if is_empty(path) {
            report.push_fix(
                id,
                label,
                Level::Fail,
                "empty",
                format!("./scripts/relogin-identity.sh {id}"),
            );
        }
    }

    if ca.is_file() && !is_empty(&ca) {
        match probe::read_pem(&ca) {
            Ok(info) if info.certificates == 0 => report.push_fix(
                id,
                "mtls ca.crt",
                Level::Fail,
                "contains no certificates",
                format!("./scripts/relogin-identity.sh {id}"),
            ),
            Ok(info) => {
                // On the ACME path the Route serves a Let's Encrypt cert
                // while the CLI's gRPC channel pins trust to this file and
                // never falls back to the system store, so the issuing
                // chain has to be appended to the chart's own CA — two
                // certificates minimum. One means the append step was
                // skipped, and every CLI call fails with UnknownIssuer.
                if layout.env.letsencrypt_issuer().is_some() && info.certificates < 2 {
                    report.push_fix(
                        id,
                        "mtls ca.crt",
                        Level::Warn,
                        format!(
                            "only {} certificate — the Route's Let's Encrypt chain looks un-appended",
                            info.certificates
                        ),
                        format!("./scripts/relogin-identity.sh {id}  # appends the chain for you"),
                    );
                } else {
                    report.push(
                        id,
                        "mtls ca.crt",
                        Level::Ok,
                        format!(
                            "{} certificate{}",
                            info.certificates,
                            if info.certificates == 1 { "" } else { "s" }
                        ),
                    );
                }
            }
            Err(e) => report.push(id, "mtls ca.crt", Level::Warn, e),
        }
    }

    if crt.is_file() && !is_empty(&crt) {
        match probe::read_pem(&crt) {
            Ok(info) => {
                let subject = info.first_subject_cn.unwrap_or_else(|| "?".to_string());
                match info.first_not_after {
                    Some(not_after) => {
                        let remaining = crate::fmt::seconds_until(not_after);
                        let span = crate::fmt::human_duration(remaining);
                        if remaining <= 0 {
                            report.push_fix(
                                id,
                                "mtls tls.crt",
                                Level::Fail,
                                format!("CN={subject}, expired {span} ago"),
                                format!("./scripts/relogin-identity.sh {id}  # re-pulls openshell-client-tls"),
                            );
                        } else if remaining < 7 * 86_400 {
                            report.push_fix(
                                id,
                                "mtls tls.crt",
                                Level::Warn,
                                format!("CN={subject}, expires in {span}"),
                                format!("./scripts/relogin-identity.sh {id}  # re-pulls openshell-client-tls"),
                            );
                        } else {
                            report.push(
                                id,
                                "mtls tls.crt",
                                Level::Ok,
                                format!("CN={subject}, {span} left"),
                            );
                        }
                    }
                    None => {
                        report.push(id, "mtls tls.crt", Level::Warn, "no parseable certificate")
                    }
                }
            }
            Err(e) => report.push(id, "mtls tls.crt", Level::Warn, e),
        }
    }

    if key.is_file() && !is_empty(&key) {
        match probe::looks_like_private_key(&key) {
            Ok(true) => {}
            Ok(false) => report.push_fix(
                id,
                "mtls tls.key",
                Level::Fail,
                "does not look like a PEM private key",
                format!("./scripts/relogin-identity.sh {id}"),
            ),
            Err(e) => report.push(id, "mtls tls.key", Level::Warn, e),
        }
        match file_mode(&key) {
            Some(mode) if mode & 0o077 != 0 => report.push_fix(
                id,
                "mtls tls.key",
                Level::Warn,
                format!("mode {:04o} — readable beyond its owner", mode),
                format!("chmod 600 {}", crate::fmt::tilde(&key)),
            ),
            Some(mode) => report.push(id, "mtls tls.key", Level::Ok, format!("mode {mode:04o}")),
            None => {}
        }
    }
}

/// Generated demo files are reported for completeness — none is required
/// to *start* the guide, but knowing whether a slot will carry them (and
/// whether the rendered realm matches the run you think it does) is the
/// reason they are in a slot at all.
fn check_demo_artifacts(layout: &Layout, report: &mut Report) {
    let Some(demo_dir) = &layout.demo_dir else {
        return;
    };
    for (rel, _is_dir) in layout.demo_artifacts() {
        if rel == Path::new(".env") {
            continue; // already covered by check_env, with better advice
        }
        let path = demo_dir.join(&rel);
        let label = rel.display().to_string();
        if path.exists() {
            report.push(
                "demo",
                &label,
                Level::Ok,
                "present, will be included in a slot",
            );
        } else {
            report.push("demo", &label, Level::Info, "not generated");
        }
    }
}

fn is_empty(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.len() == 0)
        .unwrap_or(true)
}

fn file_mode(path: &Path) -> Option<u32> {
    std::fs::metadata(path)
        .ok()
        .map(|m| crate::manifest::mode_of(&m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_keycloak_account_is_not_a_mismatch() {
        assert!(subject_matches("admin", "openshell-admin"));
        assert!(subject_matches("alice", "alice"));
        assert!(subject_matches("alice", "alice@example.com"));
        assert!(!subject_matches("bob", "alice"));
    }

    /// A directory suffixed to park a second cluster's logins still holds
    /// a token for the plain persona — `oc-alice-rhgaudi3s2/` is alice.
    #[test]
    fn a_cluster_suffixed_directory_is_not_a_wrong_persona() {
        assert!(subject_matches("alice-rhgaudi3s2", "alice"));
        assert!(subject_matches("bob-rhgaudi3s2", "bob"));
        assert!(subject_matches("admin-rhgaudi3s2", "openshell-admin"));
        assert!(subject_matches("charlie-sandbox341", "charlie@example.com"));
    }

    /// The case the check exists for has to survive the looseness above.
    #[test]
    fn a_token_in_the_wrong_personas_directory_is_still_caught() {
        assert!(!subject_matches("bob", "alice"));
        assert!(!subject_matches("bob-rhgaudi3s2", "alice"));
        assert!(!subject_matches("alice", "openshell-admin"));
        assert!(!subject_matches("charlie-rhgaudi3s2", "bob"));
    }

    #[test]
    fn worst_level_drives_the_exit_code() {
        let mut report = Report::default();
        report.push("env", "a", Level::Ok, "fine");
        report.push("env", "b", Level::Warn, "hmm");
        assert_eq!(report.worst(), Level::Warn);
        report.push("env", "c", Level::Fail, "no");
        assert_eq!(report.worst(), Level::Fail);
        assert_eq!(report.count(Level::Ok), 1);
    }
}
