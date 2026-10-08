// SPDX-License-Identifier: Apache-2.0

//! Slot operations: `save`, `list`, `show`, `restore`, `diff`, `rm`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::fmt;
use crate::layout::{CA_BUNDLE_NAME, Layout, Scope};
use crate::manifest::{Copier, FileEntry, IdentityEntry, Manifest, restore_path};
use crate::probe::GatewayMetadata;

pub const SLOT_IDENTITIES: &str = "identities";
pub const SLOT_SHARED: &str = "shared";
pub const SLOT_DEMO: &str = "demo";

pub struct Slot {
    pub name: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
}

pub fn slot_dir(layout: &Layout, name: &str) -> PathBuf {
    layout.slots_dir.join(name)
}

/// Every readable slot, newest first. A directory without a usable
/// manifest is reported as an error rather than hidden — a half-written
/// slot you can't see is worse than one you can't use.
pub fn list(layout: &Layout) -> Vec<Result<Slot, String>> {
    let Ok(entries) = fs::read_dir(&layout.slots_dir) else {
        return Vec::new();
    };
    let mut slots: Vec<Result<Slot, String>> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let dir = e.path();
            Manifest::load(&dir)
                .map(|manifest| Slot {
                    name: name.clone(),
                    dir,
                    manifest,
                })
                .map_err(|err| format!("{name}: {err}"))
        })
        .collect();
    slots.sort_by(|a, b| match (a, b) {
        (Ok(a), Ok(b)) => b.manifest.created_at.cmp(&a.manifest.created_at),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(a), Err(b)) => a.cmp(b),
    });
    slots
}

pub fn load(layout: &Layout, name: &str) -> Result<Slot, String> {
    let dir = slot_dir(layout, name);
    if !dir.is_dir() {
        return Err(format!(
            "no slot named '{name}' in {}",
            fmt::tilde(&layout.slots_dir)
        ));
    }
    Ok(Slot {
        name: name.to_string(),
        dir: dir.clone(),
        manifest: Manifest::load(&dir)?,
    })
}

/// Copy the current state into a new slot.
///
/// Built in a temporary directory and renamed into place, so an
/// interrupted save leaves no half-slot that `restore` would happily feed
/// back onto a working setup.
pub fn save(
    layout: &Layout,
    name: &str,
    identities: &[String],
    scopes: &[Scope],
    note: Option<String>,
    force: bool,
) -> Result<Slot, String> {
    crate::manifest::validate_slot_name(name)?;
    crate::manifest::ensure_private_dir(&layout.slots_dir)
        .map_err(|e| format!("cannot create {}: {e}", fmt::tilde(&layout.slots_dir)))?;

    let final_dir = slot_dir(layout, name);
    if final_dir.exists() && !force {
        return Err(format!(
            "slot '{name}' already exists — pass --force to overwrite it, or pick another name"
        ));
    }

    let staging = layout
        .slots_dir
        .join(format!(".staging-{name}-{}", std::process::id()));
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    let result = build_slot(layout, &staging, identities, scopes, note);
    let manifest = match result {
        Ok(manifest) => manifest,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    if final_dir.exists() {
        fs::remove_dir_all(&final_dir).map_err(|e| format!("cannot replace slot '{name}': {e}"))?;
    }
    fs::rename(&staging, &final_dir).map_err(|e| {
        let _ = fs::remove_dir_all(&staging);
        format!("cannot finalise slot '{name}': {e}")
    })?;

    Ok(Slot {
        name: name.to_string(),
        dir: final_dir,
        manifest,
    })
}

fn build_slot(
    layout: &Layout,
    staging: &Path,
    identities: &[String],
    scopes: &[Scope],
    note: Option<String>,
) -> Result<Manifest, String> {
    crate::manifest::ensure_private_dir(&staging.to_path_buf())
        .map_err(|e| format!("cannot create {}: {e}", staging.display()))?;

    let mut copier = Copier::new();
    let mut identity_entries = Vec::new();
    let mut shared = Vec::new();
    let mut demo = Vec::new();
    let mut demo_artifacts = Vec::new();

    if scopes.contains(&Scope::Identities) {
        for id in identities {
            let paths = layout.identity(id);
            if !paths.exists() {
                continue; // nothing to capture; `status` is where this is reported
            }
            let endpoint = GatewayMetadata::read(&paths.metadata())
                .ok()
                .and_then(|m| m.endpoint);
            let dst = staging.join(SLOT_IDENTITIES).join(id);
            copier
                .copy(&paths.root, &dst, &format!("{SLOT_IDENTITIES}/{id}"))
                .map_err(|e| format!("copying {}: {e}", fmt::tilde(&paths.root)))?;
            identity_entries.push(IdentityEntry {
                id: id.clone(),
                gateway_endpoint: endpoint,
                files: copier.take(),
            });
        }
    }

    if scopes.contains(&Scope::Shared) && layout.ca_bundle.is_file() {
        let dst = staging.join(SLOT_SHARED).join(CA_BUNDLE_NAME);
        copier
            .copy(
                &layout.ca_bundle,
                &dst,
                &format!("{SLOT_SHARED}/{CA_BUNDLE_NAME}"),
            )
            .map_err(|e| format!("copying {}: {e}", fmt::tilde(&layout.ca_bundle)))?;
        shared = copier.take();
    }

    if scopes.contains(&Scope::Demo)
        && let Some(demo_dir) = &layout.demo_dir
    {
        for (rel, _is_dir) in layout.demo_artifacts() {
            let src = demo_dir.join(&rel);
            if !src.exists() {
                continue;
            }
            let rel_str = rel.to_string_lossy().to_string();
            copier
                .copy(
                    &src,
                    &staging.join(SLOT_DEMO).join(&rel),
                    &format!("{SLOT_DEMO}/{rel_str}"),
                )
                .map_err(|e| format!("copying {}: {e}", fmt::tilde(&src)))?;
            demo_artifacts.push(rel_str);
        }
        demo = copier.take();
    }

    let manifest = Manifest {
        format: crate::manifest::FORMAT_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: fmt::now_rfc3339(),
        note,
        gateway: layout.gateway.clone(),
        namespace: layout.env.namespace().map(str::to_string),
        apps_domain: layout.env.apps_domain().map(str::to_string),
        expected_endpoint: layout.env.expected_endpoint(),
        scopes: scopes.iter().map(|s| s.as_str().to_string()).collect(),
        identities: identity_entries,
        shared,
        demo,
        demo_artifacts,
        skipped: std::mem::take(&mut copier.skipped),
    };
    manifest
        .store(staging)
        .map_err(|e| format!("cannot write manifest: {e}"))?;
    Ok(manifest)
}

/// One thing `restore` will do, resolved before anything is written so
/// `--dry-run` and the confirmation prompt describe the real plan.
pub struct RestoreStep {
    pub label: String,
    pub from: PathBuf,
    pub to: PathBuf,
    /// Whether the destination already exists and will be replaced.
    pub replaces: bool,
}

pub fn plan_restore(
    layout: &Layout,
    slot: &Slot,
    identities: Option<&[String]>,
    scopes: &[Scope],
) -> Result<Vec<RestoreStep>, String> {
    let mut steps = Vec::new();

    if scopes.contains(&Scope::Identities) {
        for entry in &slot.manifest.identities {
            if let Some(filter) = identities
                && !filter.contains(&entry.id)
            {
                continue;
            }
            let from = slot.dir.join(SLOT_IDENTITIES).join(&entry.id);
            if !from.is_dir() {
                continue;
            }
            let to = layout.state_root.join(format!("oc-{}", entry.id));
            steps.push(RestoreStep {
                label: format!("identity {}", entry.id),
                replaces: to.exists(),
                from,
                to,
            });
        }
    }

    if scopes.contains(&Scope::Shared) {
        let from = slot.dir.join(SLOT_SHARED).join(CA_BUNDLE_NAME);
        if from.is_file() {
            steps.push(RestoreStep {
                label: "shared CA bundle".to_string(),
                replaces: layout.ca_bundle.exists(),
                from,
                to: layout.ca_bundle.clone(),
            });
        }
    }

    if scopes.contains(&Scope::Demo) && !slot.manifest.demo_artifacts.is_empty() {
        let demo_dir = layout.demo_dir.as_ref().ok_or_else(|| {
            "this slot carries demo files but the demo directory could not be found; \
             pass --demo-dir or re-run with --scope identities"
                .to_string()
        })?;
        for rel in &slot.manifest.demo_artifacts {
            let from = slot.dir.join(SLOT_DEMO).join(rel);
            if !from.exists() {
                continue;
            }
            let to = demo_dir.join(rel);
            steps.push(RestoreStep {
                label: format!("demo {rel}"),
                replaces: to.exists(),
                from,
                to,
            });
        }
    }

    Ok(steps)
}

pub fn apply_restore(steps: &[RestoreStep]) -> Result<(), String> {
    for step in steps {
        restore_path(&step.from, &step.to)
            .map_err(|e| format!("restoring {} to {}: {e}", step.label, fmt::tilde(&step.to)))?;
    }
    Ok(())
}

#[derive(PartialEq, Eq, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffState {
    Same,
    /// Present in both, different content.
    Changed,
    /// In the slot, absent on disk.
    OnlyInSlot,
    /// On disk, absent from the slot.
    OnlyLive,
}

impl DiffState {
    pub fn marker(self) -> &'static str {
        match self {
            DiffState::Same => "=",
            DiffState::Changed => "~",
            DiffState::OnlyInSlot => "-",
            DiffState::OnlyLive => "+",
        }
    }
}

#[derive(serde::Serialize)]
pub struct DiffEntry {
    /// Slot-relative path, which doubles as a stable label for the file.
    pub path: String,
    pub state: DiffState,
}

/// Compare a slot against what is on disk right now.
///
/// Hashes are recomputed for live files only; the slot's own hashes come
/// from its manifest, so a diff never reads the saved secrets.
pub fn diff(
    layout: &Layout,
    slot: &Slot,
    identities: Option<&[String]>,
    scopes: &[Scope],
) -> Vec<DiffEntry> {
    let mut entries = Vec::new();
    let mut in_slot: BTreeSet<String> = BTreeSet::new();

    let wanted = |path: &str| -> bool {
        let scope_ok = match path.split('/').next() {
            Some(SLOT_IDENTITIES) => scopes.contains(&Scope::Identities),
            Some(SLOT_SHARED) => scopes.contains(&Scope::Shared),
            Some(SLOT_DEMO) => scopes.contains(&Scope::Demo),
            _ => false,
        };
        if !scope_ok {
            return false;
        }
        match (identities, identity_of(path)) {
            (Some(filter), Some(id)) => filter.iter().any(|f| f == id),
            _ => true,
        }
    };

    let saved: Vec<&FileEntry> = slot
        .manifest
        .identities
        .iter()
        .flat_map(|i| i.files.iter())
        .chain(slot.manifest.shared.iter())
        .chain(slot.manifest.demo.iter())
        .filter(|f| wanted(&f.path))
        .collect();

    for file in saved {
        in_slot.insert(file.path.clone());
        let Some(live) = live_path_for(layout, &file.path) else {
            continue;
        };
        let state = match live.is_file() {
            false => DiffState::OnlyInSlot,
            true => match crate::manifest::hash_file(&live) {
                Ok(hash) if hash == file.sha256 => DiffState::Same,
                _ => DiffState::Changed,
            },
        };
        entries.push(DiffEntry {
            path: file.path.clone(),
            state,
        });
    }

    for path in live_paths(layout, identities, scopes) {
        if !in_slot.contains(&path) {
            entries.push(DiffEntry {
                path,
                state: DiffState::OnlyLive,
            });
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

/// `identities/<id>/…` → `<id>`.
fn identity_of(slot_rel: &str) -> Option<&str> {
    let mut parts = slot_rel.split('/');
    if parts.next()? != SLOT_IDENTITIES {
        return None;
    }
    parts.next()
}

/// Map a slot-relative path back to where it lives on a running machine.
fn live_path_for(layout: &Layout, slot_rel: &str) -> Option<PathBuf> {
    let (head, rest) = slot_rel.split_once('/')?;
    match head {
        SLOT_IDENTITIES => {
            let (id, tail) = rest.split_once('/')?;
            Some(layout.state_root.join(format!("oc-{id}")).join(tail))
        }
        SLOT_SHARED => (rest == CA_BUNDLE_NAME).then(|| layout.ca_bundle.clone()),
        SLOT_DEMO => layout.demo_dir.as_ref().map(|d| d.join(rest)),
        _ => None,
    }
}

/// Every live file that *would* be captured, as slot-relative paths — the
/// other half of the diff, so files added since the save show up as `+`.
fn live_paths(layout: &Layout, identities: Option<&[String]>, scopes: &[Scope]) -> Vec<String> {
    let mut out = Vec::new();

    if scopes.contains(&Scope::Identities) {
        let ids: Vec<String> = match identities {
            Some(filter) => filter.to_vec(),
            None => layout.discover_identities(),
        };
        for id in ids {
            let root = layout.identity(&id).root;
            walk(&root, &format!("{SLOT_IDENTITIES}/{id}"), &mut out);
        }
    }
    if scopes.contains(&Scope::Shared) && layout.ca_bundle.is_file() {
        out.push(format!("{SLOT_SHARED}/{CA_BUNDLE_NAME}"));
    }
    if scopes.contains(&Scope::Demo)
        && let Some(demo_dir) = &layout.demo_dir
    {
        for (rel, _) in layout.demo_artifacts() {
            let rel_str = rel.to_string_lossy().to_string();
            walk(
                &demo_dir.join(&rel),
                &format!("{SLOT_DEMO}/{rel_str}"),
                &mut out,
            );
        }
    }
    out
}

fn walk(path: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_file() {
        out.push(rel.to_string());
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        walk(&entry.path(), &format!("{rel}/{name}"), out);
    }
}

pub fn remove(layout: &Layout, name: &str) -> Result<(), String> {
    crate::manifest::validate_slot_name(name)?;
    let dir = slot_dir(layout, name);
    if !dir.is_dir() {
        return Err(format!("no slot named '{name}'"));
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove slot '{name}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_extracted_from_slot_paths() {
        assert_eq!(
            identity_of("identities/alice/config/openshell/gateways/openshift/metadata.json"),
            Some("alice")
        );
        assert_eq!(identity_of("demo/.env"), None);
        assert_eq!(identity_of("shared/ca.pem"), None);
    }
}
