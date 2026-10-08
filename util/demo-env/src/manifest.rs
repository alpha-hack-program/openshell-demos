// SPDX-License-Identifier: Apache-2.0

//! The on-disk shape of a slot, and the copy/hash machinery behind
//! `save` and `restore`.
//!
//! ```text
//! <slots-dir>/<slot>/
//! ├── manifest.json                     # everything below, described
//! ├── identities/<id>/{config,state}/…  # verbatim oc-<id> tree
//! ├── shared/keycloak-oidc-ca-bundle.pem
//! └── demo/{.env,keycloak/…,…}
//! ```
//!
//! Two properties are deliberate. First, the payload is a **verbatim copy
//! of the whole tree**, not just the handful of files `status` knows how to
//! check — the openshell CLI is free to add state files between versions,
//! and a restore that silently dropped them would be worse than no restore
//! at all. Second, `manifest.json` records a hash for every file, so
//! `list`/`show`/`diff` never have to read the payload, which means they
//! never have to touch the secrets inside it.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MANIFEST_NAME: &str = "manifest.json";

/// Bumped only on a breaking change to the layout above. `show`/`restore`
/// refuse a slot from the future rather than guessing.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// `demo-env` version that wrote the slot, for support questions.
    pub tool_version: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Gateway registration name the identities were saved under.
    pub gateway: String,
    /// What `.env` said at save time. Recorded so `list` can show which
    /// cluster a slot belongs to without opening the payload — the single
    /// most useful thing to know when picking a slot to restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_endpoint: Option<String>,
    /// Which scopes this slot actually carries. A slot saved with
    /// `--scope identities` can't restore a `.env` it never captured, and
    /// `restore` says so instead of quietly doing less than asked.
    pub scopes: Vec<String>,
    pub identities: Vec<IdentityEntry>,
    #[serde(default)]
    pub shared: Vec<FileEntry>,
    #[serde(default)]
    pub demo: Vec<FileEntry>,
    /// Top-level demo paths captured, relative to `demos/keycloak-oidc`
    /// (`.env`, `onboarding-web-admin-session`, …).
    ///
    /// `demo` above lists individual files, which is the wrong granularity
    /// for restoring: putting a directory back means replacing *that
    /// directory*, and the one thing restore must never do is replace the
    /// demo directory itself. This records exactly which top-level paths
    /// the slot owns, so nothing outside them is ever touched.
    #[serde(default)]
    pub demo_artifacts: Vec<String>,
    /// Anything skipped while copying (symlinks, unreadable files), so a
    /// partial save is visible rather than silent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

impl Manifest {
    pub fn load(slot_dir: &Path) -> Result<Manifest, String> {
        let path = slot_dir.join(MANIFEST_NAME);
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a valid manifest: {e}", path.display()))?;
        if manifest.format > FORMAT_VERSION {
            return Err(format!(
                "{} was written by a newer demo-env (format {}, this build understands {}); upgrade the tool",
                path.display(),
                manifest.format,
                FORMAT_VERSION
            ));
        }
        Ok(manifest)
    }

    pub fn store(&self, slot_dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self).expect("manifest serialises");
        fs::write(slot_dir.join(MANIFEST_NAME), text + "\n")
    }

    pub fn has_scope(&self, scope: crate::layout::Scope) -> bool {
        self.scopes.iter().any(|s| s == scope.as_str())
    }

    pub fn file_count(&self) -> usize {
        self.identities.iter().map(|i| i.files.len()).sum::<usize>()
            + self.shared.len()
            + self.demo.len()
    }

    pub fn total_bytes(&self) -> u64 {
        self.identities
            .iter()
            .flat_map(|i| i.files.iter())
            .chain(self.shared.iter())
            .chain(self.demo.iter())
            .map(|f| f.size)
            .sum()
    }
}

#[derive(Serialize, Deserialize)]
pub struct IdentityEntry {
    pub id: String,
    /// `gateway_endpoint` from this identity's `metadata.json` at save
    /// time. Lets `list` flag a slot whose personas were registered
    /// against different clusters — a state the guide can never work in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_endpoint: Option<String>,
    pub files: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FileEntry {
    /// Path relative to the slot directory.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// Unix permission bits, restored verbatim — `tls.key` arriving back
    /// as 0644 would be a quiet downgrade.
    pub mode: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime: Option<String>,
}

/// Walks a tree, copying it into the slot and recording an entry per file.
pub struct Copier {
    pub entries: Vec<FileEntry>,
    pub skipped: Vec<String>,
}

impl Copier {
    pub fn new() -> Copier {
        Copier {
            entries: Vec::new(),
            skipped: Vec::new(),
        }
    }

    /// Copy `src` (file or directory) to `dst`, recording entries under
    /// `rel_prefix` (the slot-relative path of `dst`).
    pub fn copy(&mut self, src: &Path, dst: &Path, rel_prefix: &str) -> io::Result<()> {
        // `symlink_metadata`, not `metadata`: a symlink here would be
        // copied as its target and silently restored as a real file.
        let meta = fs::symlink_metadata(src)?;
        if meta.file_type().is_symlink() {
            self.skipped.push(format!("{} (symlink)", src.display()));
            return Ok(());
        }
        if meta.is_dir() {
            fs::create_dir_all(dst)?;
            let mut children: Vec<_> = fs::read_dir(src)?.collect::<Result<_, _>>()?;
            children.sort_by_key(|e| e.file_name());
            for child in children {
                let name = child.file_name();
                let name_str = name.to_string_lossy();
                self.copy(
                    &src.join(&name),
                    &dst.join(&name),
                    &format!("{rel_prefix}/{name_str}"),
                )?;
            }
            return Ok(());
        }
        if !meta.is_file() {
            self.skipped
                .push(format!("{} (not a regular file)", src.display()));
            return Ok(());
        }

        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst)?;
        let mode = mode_of(&meta);
        fs::set_permissions(dst, permissions_from_mode(mode))?;
        self.entries.push(FileEntry {
            path: rel_prefix.to_string(),
            size: meta.len(),
            sha256: hash_file(src)?,
            mode,
            mtime: meta.modified().ok().map(crate::fmt::rfc3339),
        });
        Ok(())
    }

    /// Entries recorded since the last `take`, leaving the copier reusable
    /// for the next scope.
    pub fn take(&mut self) -> Vec<FileEntry> {
        std::mem::take(&mut self.entries)
    }
}

/// Restore `src` onto `dst`.
///
/// Directories are **replaced**, not merged: a leftover
/// `gateways/<other-cluster>/` from the live tree surviving a restore is
/// precisely the stale-registration failure this tool exists to catch, so
/// the destination is removed first.
pub fn restore_path(src: &Path, dst: &Path) -> io::Result<()> {
    if dst.exists() || fs::symlink_metadata(dst).is_ok() {
        if fs::symlink_metadata(dst)?.is_dir() {
            fs::remove_dir_all(dst)?;
        } else {
            fs::remove_file(dst)?;
        }
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    copy_back(src, dst)
}

fn copy_back(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() {
        fs::create_dir_all(dst)?;
        fs::set_permissions(dst, permissions_from_mode(mode_of(&meta)))?;
        for child in fs::read_dir(src)? {
            let child = child?;
            copy_back(&child.path(), &dst.join(child.file_name()))?;
        }
    } else if meta.is_file() {
        fs::copy(src, dst)?;
        fs::set_permissions(dst, permissions_from_mode(mode_of(&meta)))?;
    }
    Ok(())
}

pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn mode_of(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o7777
}

fn permissions_from_mode(mode: u32) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    fs::Permissions::from_mode(mode)
}

/// Slot names become directory names, so they have to be boring.
pub fn validate_slot_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("slot name cannot be empty".to_string());
    }
    if name.starts_with('.') {
        return Err("slot name cannot start with '.'".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!(
            "slot name '{name}' may only contain letters, digits, '-', '_' and '.'"
        ));
    }
    Ok(())
}

/// Create the slots root with owner-only permissions. Everything written
/// under it — refresh tokens, `tls.key`, the demo `.env`'s API keys — is
/// material a slot should never expose to other local accounts.
pub fn ensure_private_dir(path: &PathBuf) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, permissions_from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_slot_names() {
        assert!(validate_slot_name("after-onboarding").is_ok());
        assert!(validate_slot_name("run.2026-10-08_1").is_ok());
        assert!(validate_slot_name("").is_err());
        assert!(validate_slot_name(".hidden").is_err());
        assert!(validate_slot_name("../escape").is_err());
        assert!(validate_slot_name("has/slash").is_err());
    }
}
