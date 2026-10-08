// SPDX-License-Identifier: Apache-2.0

//! Where everything lives.
//!
//! The keycloak-oidc guide spreads one logical "demo state" across three
//! unrelated places on disk, and nothing in the repo lists them in one
//! spot — that's what this module is. The three:
//!
//! 1. **Per-identity CLI state**, one tree per persona, under
//!    `$HOME/.local/state/openshell-demos/oc-<id>/{config,state}`. This is
//!    the `XDG_CONFIG_HOME`/`XDG_STATE_HOME` pair that
//!    `scripts/lib-use-identity.sh` exports — an "identity" in this demo is
//!    nothing more than a gateway registration living under one of these.
//! 2. **One shared ingress-CA bundle** at
//!    `${XDG_CACHE_HOME:-$HOME/.cache}/openshell-demos/keycloak-oidc-ca-bundle.pem`.
//!    Deliberately *not* identity-scoped (see the README's "OIDC issuer TLS
//!    trust" section) — every persona's `SSL_CERT_FILE` points at this one
//!    file.
//! 3. **Demo-local generated files** inside the checkout:
//!    `demos/keycloak-oidc/.env` plus the gitignored artifacts the scripts
//!    render next to it.
//!
//! All three are gitignored or outside the repo entirely, so a fresh clone
//! plus a `git status` tells you nothing about whether the demo is ready to
//! run. Hence this tool.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The personas the guide is written around, in the order the README
/// introduces them (admin first — it's Terminal A, and every banker's mTLS
/// material is copied from it by `scripts/relogin-identity.sh`).
pub const CANONICAL_IDENTITIES: [&str; 4] = ["admin", "alice", "bob", "charlie"];

/// Default gateway registration name, matching `GATEWAY_NAME` in the demo
/// scripts.
pub const DEFAULT_GATEWAY: &str = "openshift";

/// Filename of the shared ingress-CA bundle, under
/// `<cache>/openshell-demos/`.
pub const CA_BUNDLE_NAME: &str = "keycloak-oidc-ca-bundle.pem";

/// Which of the three storage areas a file belongs to. Used to let
/// `save`/`restore`/`status` operate on a subset — restoring a slot's
/// identities without clobbering the `.env` in your working tree is the
/// common case.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum Scope {
    /// Per-persona `oc-<id>/{config,state}` trees.
    Identities,
    /// The shared ingress-CA bundle.
    Shared,
    /// Generated files inside `demos/keycloak-oidc/`, including `.env`.
    Demo,
}

impl Scope {
    pub fn all() -> Vec<Scope> {
        vec![Scope::Identities, Scope::Shared, Scope::Demo]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Identities => "identities",
            Scope::Shared => "shared",
            Scope::Demo => "demo",
        }
    }
}

/// Fully resolved paths plus the demo `.env` that gives them meaning.
pub struct Layout {
    /// `$HOME/.local/state/openshell-demos` — parent of every `oc-<id>`.
    pub state_root: PathBuf,
    /// Where slots are written. Outside the repo on purpose: a slot holds
    /// refresh tokens, a TLS private key and the demo `.env`'s API keys.
    pub slots_dir: PathBuf,
    /// The one shared ingress-CA bundle.
    pub ca_bundle: PathBuf,
    /// `<checkout>/demos/keycloak-oidc`, if we could find it.
    pub demo_dir: Option<PathBuf>,
    /// Gateway registration name (`GATEWAY_NAME`, default `openshift`).
    pub gateway: String,
    /// Parsed `demos/keycloak-oidc/.env`.
    pub env: DemoEnv,
}

impl Layout {
    pub fn resolve(
        demo_dir: Option<PathBuf>,
        state_root: Option<PathBuf>,
        slots_dir: Option<PathBuf>,
        gateway: Option<String>,
    ) -> Result<Layout, String> {
        let home = PathBuf::from(
            std::env::var("HOME").map_err(|_| "HOME is not set; cannot locate demo state")?,
        );

        let demo_dir = match demo_dir {
            Some(d) => Some(d),
            None => find_demo_dir(),
        };
        let env = match &demo_dir {
            Some(d) => DemoEnv::load(&d.join(".env")),
            None => DemoEnv::empty(),
        };

        let state_root = state_root.unwrap_or_else(|| home.join(".local/state/openshell-demos"));
        let slots_dir = slots_dir.unwrap_or_else(|| state_root.join("snapshots"));

        // Mirrors lib-use-identity.sh: $XDG_CACHE_HOME, else ~/.cache.
        let cache_home = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| home.join(".cache"));

        // CLI flag wins, then the .env the demo scripts themselves read,
        // then the same default those scripts fall back to.
        let gateway = gateway
            .or_else(|| env.get("GATEWAY_NAME").map(str::to_string))
            .unwrap_or_else(|| DEFAULT_GATEWAY.to_string());

        Ok(Layout {
            state_root,
            slots_dir,
            ca_bundle: cache_home.join("openshell-demos").join(CA_BUNDLE_NAME),
            demo_dir,
            gateway,
            env,
        })
    }

    /// Paths for one persona.
    pub fn identity(&self, id: &str) -> IdentityPaths {
        let root = self.state_root.join(format!("oc-{id}"));
        IdentityPaths {
            config_home: root.join("config"),
            state_home: root.join("state"),
            gateway: self.gateway.clone(),
            root,
        }
    }

    /// The canonical four, plus any other `oc-*` directory someone has
    /// created. Discovering extras matters because the guide's onboarding
    /// is parameterised by user id — a demo run with a fifth banker would
    /// otherwise be silently excluded from every snapshot.
    pub fn discover_identities(&self) -> Vec<String> {
        let mut found: Vec<String> = CANONICAL_IDENTITIES.iter().map(|s| s.to_string()).collect();
        if let Ok(entries) = std::fs::read_dir(&self.state_root) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let Some(id) = name.strip_prefix("oc-") else {
                    continue;
                };
                if !id.is_empty() && !found.iter().any(|f| f == id) {
                    found.push(id.to_string());
                }
            }
        }
        // admin first (Terminal A), the rest alphabetical.
        let (admin, mut rest): (Vec<_>, Vec<_>) = found.into_iter().partition(|id| id == "admin");
        rest.sort();
        admin.into_iter().chain(rest).collect()
    }

    /// Generated, gitignored files inside the demo directory that are worth
    /// carrying in a slot. All optional — a demo run that never reached
    /// step 1a or never deployed openclaw simply won't have them.
    ///
    /// Returns `(relative path, is_directory)` pairs.
    pub fn demo_artifacts(&self) -> Vec<(PathBuf, bool)> {
        let Some(demo_dir) = &self.demo_dir else {
            return Vec::new();
        };
        let mut out = vec![
            (PathBuf::from(".env"), false),
            (PathBuf::from("keycloak/realm-export.rendered.json"), false),
            (PathBuf::from("onboarding-web-admin-session"), true),
        ];
        // `openclaw-<user>-session/` — one per persona who ran Annex H, so
        // the set isn't known up front. Matched by prefix/suffix rather
        // than pulling in a glob dependency for one pattern.
        if let Ok(entries) = std::fs::read_dir(demo_dir) {
            let mut sessions: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("openclaw-") && n.ends_with("-session"))
                .map(PathBuf::from)
                .collect();
            sessions.sort();
            out.extend(sessions.into_iter().map(|p| (p, true)));
        }
        out
    }
}

/// Paths for a single persona's CLI state.
pub struct IdentityPaths {
    /// `oc-<id>` — the whole tree a slot copies.
    pub root: PathBuf,
    /// What `use_identity` exports as `XDG_CONFIG_HOME`.
    pub config_home: PathBuf,
    /// What `use_identity` exports as `XDG_STATE_HOME`.
    pub state_home: PathBuf,
    gateway: String,
}

impl IdentityPaths {
    pub fn gateway_dir(&self) -> PathBuf {
        self.config_home
            .join("openshell/gateways")
            .join(&self.gateway)
    }

    /// Gateway registration. Its `gateway_endpoint` is what
    /// `use_identity` cross-checks against `.env` to catch a login left
    /// over from a different cluster.
    pub fn metadata(&self) -> PathBuf {
        self.gateway_dir().join("metadata.json")
    }

    /// The OIDC bundle `openshell gateway login` writes and `parrot`
    /// rotates — access token, refresh token, issuer, client id.
    pub fn oidc_token(&self) -> PathBuf {
        self.gateway_dir().join("oidc_token.json")
    }

    pub fn mtls_dir(&self) -> PathBuf {
        self.gateway_dir().join("mtls")
    }

    pub fn ca_crt(&self) -> PathBuf {
        self.mtls_dir().join("ca.crt")
    }

    pub fn tls_crt(&self) -> PathBuf {
        self.mtls_dir().join("tls.crt")
    }

    pub fn tls_key(&self) -> PathBuf {
        self.mtls_dir().join("tls.key")
    }

    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }
}

/// The demo's `.env`, parsed the way `set -a; source .env` would see it.
pub struct DemoEnv {
    pub path: Option<PathBuf>,
    vars: BTreeMap<String, String>,
}

impl DemoEnv {
    fn empty() -> DemoEnv {
        DemoEnv {
            path: None,
            vars: BTreeMap::new(),
        }
    }

    fn load(path: &Path) -> DemoEnv {
        let Ok(text) = std::fs::read_to_string(path) else {
            return DemoEnv {
                path: Some(path.to_path_buf()),
                vars: BTreeMap::new(),
            };
        };
        DemoEnv {
            path: Some(path.to_path_buf()),
            vars: parse_env(&text),
        }
    }

    /// Treats an empty value as absent — `.env.example` ships keys like
    /// `CLUSTER_APPS_DOMAIN=` with no value, and a copied-but-unfilled
    /// `.env` should read as "not configured", not "configured to empty".
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    pub fn namespace(&self) -> Option<&str> {
        self.get("OPENSHELL_NAMESPACE")
    }

    pub fn apps_domain(&self) -> Option<&str> {
        self.get("CLUSTER_APPS_DOMAIN")
    }

    pub fn letsencrypt_issuer(&self) -> Option<&str> {
        self.get("LETSENCRYPT_CLUSTER_ISSUER")
    }

    /// `openshell-<namespace>.<apps-domain>` — the Route hostname every
    /// script derives with this same formula.
    pub fn route_host(&self) -> Option<String> {
        Some(format!(
            "openshell-{}.{}",
            self.namespace()?,
            self.apps_domain()?
        ))
    }

    /// The endpoint a correct gateway registration must carry. Same string
    /// `lib-use-identity.sh` builds before comparing it to `metadata.json`.
    pub fn expected_endpoint(&self) -> Option<String> {
        Some(format!("https://{}:443", self.route_host()?))
    }
}

/// Parse `.env` the way bash's `source` would, for the subset of syntax
/// these files actually use: `KEY=VALUE`, `#` comments (whole-line and
/// trailing-after-whitespace), and optional surrounding quotes.
///
/// Trailing comments matter here: `.env.example` is full of lines like
/// `OPENSHELL_NAMESPACE=keycloak-oidc-demo   # ...`, and a naive
/// split-on-`=` would hand back a namespace with a comment glued to it,
/// which then fails the endpoint cross-check for a reason nobody could
/// guess from the output.
fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        vars.insert(key.to_string(), strip_value(rest));
    }
    vars
}

/// Cut a trailing unquoted comment, then unquote. A `#` only starts a
/// comment when it follows whitespace (or opens the value) and is outside
/// quotes — exactly bash's rule, so `PASSWORD=a#b` keeps its `#`.
fn strip_value(rest: &str) -> String {
    let mut quote: Option<char> = None;
    let mut prev_ws = true;
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                prev_ws = false;
            }
            None => {
                if c == '\'' || c == '"' {
                    quote = Some(c);
                    prev_ws = false;
                } else if c == '#' && prev_ws {
                    end = i;
                    break;
                } else {
                    prev_ws = c.is_whitespace();
                }
            }
        }
    }
    let value = rest[..end].trim();
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

/// Walk up from the current directory looking for the checkout. Anchored on
/// `.env.example` rather than the directory alone so that a stray empty
/// `demos/keycloak-oidc` elsewhere can't win.
fn find_demo_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("demos/keycloak-oidc");
        if candidate.join(".env.example").is_file() {
            return Some(candidate);
        }
        // Also handle being *inside* the demo directory already.
        if dir.join(".env.example").is_file()
            && dir.file_name().is_some_and(|n| n == "keycloak-oidc")
        {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_trailing_comments_like_bash() {
        let env = parse_env(
            "OPENSHELL_NAMESPACE=keycloak-oidc-demo   # short slug, no openshell- prefix\n",
        );
        assert_eq!(
            env.get("OPENSHELL_NAMESPACE").map(String::as_str),
            Some("keycloak-oidc-demo")
        );
    }

    #[test]
    fn keeps_hash_inside_a_value() {
        let env = parse_env("KEYCLOAK_CLIENT_SECRET=a#b\n");
        assert_eq!(
            env.get("KEYCLOAK_CLIENT_SECRET").map(String::as_str),
            Some("a#b")
        );
    }

    #[test]
    fn unquotes_and_skips_comments_and_blanks() {
        let env = parse_env("# comment\n\nexport FOO=\"bar baz\"\nBARE=\n");
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar baz"));
        assert_eq!(env.get("BARE").map(String::as_str), Some(""));
    }

    #[test]
    fn empty_values_read_as_absent() {
        let env = DemoEnv {
            path: None,
            vars: parse_env("CLUSTER_APPS_DOMAIN=\nOPENSHELL_NAMESPACE=demo\n"),
        };
        assert_eq!(env.apps_domain(), None);
        assert_eq!(env.namespace(), Some("demo"));
        assert_eq!(env.expected_endpoint(), None);
    }

    #[test]
    fn derives_the_same_endpoint_the_shell_scripts_do() {
        let env = DemoEnv {
            path: None,
            vars: parse_env(
                "OPENSHELL_NAMESPACE=keycloak-oidc-demo\nCLUSTER_APPS_DOMAIN=apps.ocp.example.com\n",
            ),
        };
        assert_eq!(
            env.expected_endpoint().as_deref(),
            Some("https://openshell-keycloak-oidc-demo.apps.ocp.example.com:443")
        );
    }
}
