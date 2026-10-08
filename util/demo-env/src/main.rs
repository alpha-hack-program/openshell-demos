// SPDX-License-Identifier: Apache-2.0

//! `demo-env` — inspect, snapshot and restore the local files the
//! `demos/keycloak-oidc` guide runs on.
//!
//! After onboarding, `source scripts/as.sh <id>` switches the current
//! shell between admin/alice/bob/charlie by re-pointing
//! `XDG_CONFIG_HOME`/`XDG_STATE_HOME`. That works, but it is write-only:
//! nothing tells you whether those directories are actually in a state the
//! guide can run from, and nothing lets you keep more than one of them at
//! a time. Re-running the guide against a second cluster silently
//! overwrites the first run's logins, because the identity directories
//! live under `$HOME` and are shared by every checkout on the machine.
//!
//! So: `status` answers "are the files in order", `where` answers "which
//! files, exactly", and `save`/`list`/`restore` keep one complete set of
//! them per cluster or per demo run.

mod fmt;
mod layout;
mod manifest;
mod probe;
mod slots;
mod status;

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use layout::{Layout, Scope};
use status::Level;

#[derive(Parser)]
#[command(
    name = "demo-env",
    version,
    about = "Inspect and snapshot the local files the keycloak-oidc demo runs on",
    long_about = "Inspect and snapshot the local files the demos/keycloak-oidc guide runs on.\n\n\
                  A \"slot\" is one complete demo state: every persona's openshell CLI\n\
                  registration and OIDC session, the shared ingress-CA bundle, and the\n\
                  demo's own generated files. Save one per cluster or per run and switch\n\
                  between them instead of re-running every browser login."
)]
struct Cli {
    /// Path to demos/keycloak-oidc (default: found by walking up from the current directory)
    #[arg(long, global = true, env = "DEMO_ENV_DEMO_DIR", value_name = "PATH")]
    demo_dir: Option<PathBuf>,

    /// Parent of the oc-<id> identity trees (default: ~/.local/state/openshell-demos)
    #[arg(long, global = true, env = "DEMO_ENV_STATE_ROOT", value_name = "PATH")]
    state_root: Option<PathBuf>,

    /// Where slots are stored (default: <state-root>/snapshots)
    #[arg(long, global = true, env = "DEMO_ENV_SLOTS_DIR", value_name = "PATH")]
    slots_dir: Option<PathBuf>,

    /// Gateway registration name (default: GATEWAY_NAME from .env, else "openshift")
    #[arg(long, global = true, value_name = "NAME")]
    gateway: Option<String>,

    /// Machine-readable output
    #[arg(long, global = true)]
    json: bool,

    /// Never emit ANSI colour (also honours NO_COLOR)
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show where every file the demo depends on lives, and whether it is there
    Where {
        #[command(flatten)]
        selection: Selection,
    },
    /// Check that the files are in order and usable
    Status {
        #[command(flatten)]
        selection: Selection,
        /// Treat warnings as failures (exit non-zero on either)
        #[arg(long)]
        strict: bool,
    },
    /// Copy the current state into a named slot
    Save {
        /// Slot name, e.g. after-onboarding
        name: String,
        #[command(flatten)]
        selection: Selection,
        /// Free-text reminder of what this slot is
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        /// Overwrite an existing slot of the same name
        #[arg(long)]
        force: bool,
    },
    /// List saved slots
    List,
    /// Show one slot in detail
    Show {
        /// Slot name
        name: String,
    },
    /// Put a slot's files back, replacing what is there now
    Restore {
        /// Slot name
        name: String,
        #[command(flatten)]
        selection: Selection,
        /// Skip the confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
        /// Print what would be replaced and stop
        #[arg(long)]
        dry_run: bool,
        /// Do not snapshot the current state into an autosave slot first
        #[arg(long)]
        no_backup: bool,
    },
    /// Compare a slot against the files on disk right now
    Diff {
        /// Slot name
        name: String,
        #[command(flatten)]
        selection: Selection,
        /// Include unchanged files
        #[arg(long)]
        all: bool,
    },
    /// Delete a slot
    Rm {
        /// Slot name
        name: String,
        /// Skip the confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Args, Clone)]
struct Selection {
    /// Limit to one persona; repeatable (default: all of them)
    #[arg(short = 'i', long = "identity", value_name = "ID")]
    identities: Vec<String>,

    /// Limit to one storage area; repeatable (default: all three)
    #[arg(long = "scope", value_enum, value_name = "SCOPE")]
    scopes: Vec<Scope>,
}

impl Selection {
    /// Personas to act on, falling back to everything discoverable.
    fn identities(&self, layout: &Layout) -> Vec<String> {
        if self.identities.is_empty() {
            layout.discover_identities()
        } else {
            self.identities.clone()
        }
    }

    /// `None` when the user named no persona — the distinction matters for
    /// `restore`/`diff`, where "all of them" means "whatever the slot
    /// holds", not "the four this machine happens to know about".
    fn identity_filter(&self) -> Option<&[String]> {
        (!self.identities.is_empty()).then_some(self.identities.as_slice())
    }

    fn scopes(&self) -> Vec<Scope> {
        if self.scopes.is_empty() {
            Scope::all()
        } else {
            let mut scopes = self.scopes.clone();
            scopes.sort();
            scopes.dedup();
            scopes
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    fmt::init_color(cli.no_color || cli.json);

    let layout = match Layout::resolve(
        cli.demo_dir.clone(),
        cli.state_root.clone(),
        cli.slots_dir.clone(),
        cli.gateway.clone(),
    ) {
        Ok(layout) => layout,
        Err(e) => return fail(&e),
    };

    let result = match &cli.command {
        Command::Where { selection } => cmd_where(&layout, selection, cli.json),
        Command::Status { selection, strict } => cmd_status(&layout, selection, *strict, cli.json),
        Command::Save {
            name,
            selection,
            note,
            force,
        } => cmd_save(&layout, name, selection, note.clone(), *force, cli.json),
        Command::List => cmd_list(&layout, cli.json),
        Command::Show { name } => cmd_show(&layout, name, cli.json),
        Command::Restore {
            name,
            selection,
            yes,
            dry_run,
            no_backup,
        } => cmd_restore(
            &layout, name, selection, *yes, *dry_run, *no_backup, cli.json,
        ),
        Command::Diff {
            name,
            selection,
            all,
        } => cmd_diff(&layout, name, selection, *all, cli.json),
        Command::Rm { name, yes } => cmd_rm(&layout, name, *yes),
    };

    match result {
        Ok(code) => code,
        Err(e) => fail(&e),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{} {message}", fmt::red("error:"));
    ExitCode::FAILURE
}

// --- where -----------------------------------------------------------

fn cmd_where(layout: &Layout, selection: &Selection, json: bool) -> Result<ExitCode, String> {
    let identities = selection.identities(layout);
    let scopes = selection.scopes();

    if json {
        let mut root = serde_json::Map::new();
        root.insert(
            "demo_dir".into(),
            layout
                .demo_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .into(),
        );
        root.insert(
            "state_root".into(),
            layout.state_root.display().to_string().into(),
        );
        root.insert(
            "slots_dir".into(),
            layout.slots_dir.display().to_string().into(),
        );
        root.insert(
            "ca_bundle".into(),
            layout.ca_bundle.display().to_string().into(),
        );
        root.insert("gateway".into(), layout.gateway.clone().into());
        let mut ids = serde_json::Map::new();
        for id in &identities {
            let paths = layout.identity(id);
            ids.insert(
                id.clone(),
                serde_json::json!({
                    "root": paths.root.display().to_string(),
                    "xdg_config_home": paths.config_home.display().to_string(),
                    "xdg_state_home": paths.state_home.display().to_string(),
                    "exists": paths.exists(),
                    "metadata": paths.metadata().is_file(),
                    "oidc_token": paths.oidc_token().is_file(),
                    "mtls": paths.mtls_dir().is_dir(),
                }),
            );
        }
        root.insert("identities".into(), serde_json::Value::Object(ids));
        println!("{}", serde_json::Value::Object(root));
        return Ok(ExitCode::SUCCESS);
    }

    if scopes.contains(&Scope::Demo) {
        match &layout.demo_dir {
            Some(demo_dir) => {
                heading("demo files", &fmt::tilde(demo_dir));
                for (rel, _) in layout.demo_artifacts() {
                    mark_line(demo_dir.join(&rel).exists(), &rel.display().to_string(), "");
                }
            }
            None => {
                heading("demo files", "not found");
                println!(
                    "    {}",
                    fmt::dim("run from inside the checkout, or pass --demo-dir")
                );
            }
        }
        println!();
    }

    if scopes.contains(&Scope::Shared) {
        heading("shared", "every persona points SSL_CERT_FILE here");
        mark_line(
            layout.ca_bundle.is_file(),
            &fmt::tilde(&layout.ca_bundle),
            "",
        );
        println!();
    }

    if scopes.contains(&Scope::Identities) {
        heading(
            "identities",
            &format!(
                "{} · gateway '{}'",
                fmt::tilde(&layout.state_root),
                layout.gateway
            ),
        );
        // Said once rather than per persona: the XDG pair is the same
        // shape for every one of them, and repeating it four times buries
        // the file list that is the actual answer to "where".
        println!(
            "    {}",
            fmt::dim("each oc-<id>/ supplies XDG_CONFIG_HOME=…/config and XDG_STATE_HOME=…/state")
        );
        for id in &identities {
            let paths = layout.identity(id);
            println!("  {} {}", fmt::bold(id), fmt::dim(&format!("oc-{id}/")));
            if !paths.exists() {
                println!("    {} no directory — never logged in", fmt::red("✗"));
                continue;
            }
            let root = &paths.root;
            for path in [
                paths.metadata(),
                paths.oidc_token(),
                paths.ca_crt(),
                paths.tls_crt(),
                paths.tls_key(),
            ] {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                mark_line(path.is_file(), &rel, "    ");
            }
            let state_files = count_files(&paths.state_home);
            mark_line(
                paths.state_home.is_dir(),
                &format!("state/ ({state_files} file{})", plural(state_files)),
                "    ",
            );
        }
        println!();
    }

    let slot_count = slots::list(layout).len();
    heading(
        "slots",
        &format!("{} · {slot_count} saved", fmt::tilde(&layout.slots_dir)),
    );
    Ok(ExitCode::SUCCESS)
}

fn heading(title: &str, detail: &str) {
    println!("{}  {}", fmt::bold(title), fmt::dim(detail));
}

fn mark_line(present: bool, text: &str, indent: &str) {
    let mark = if present {
        fmt::green("✓")
    } else {
        fmt::dim("·")
    };
    let body = if present {
        text.to_string()
    } else {
        fmt::dim(text)
    };
    println!("{indent}  {mark} {body}");
}

fn count_files(dir: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            if e.path().is_dir() {
                count_files(&e.path())
            } else {
                1
            }
        })
        .sum()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// --- status ----------------------------------------------------------

fn cmd_status(
    layout: &Layout,
    selection: &Selection,
    strict: bool,
    json: bool,
) -> Result<ExitCode, String> {
    let report = status::run(layout, &selection.identities(layout), &selection.scopes());

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        render_report(&report);
    }

    let worst = report.worst();
    let bad = worst == Level::Fail || (strict && worst == Level::Warn);
    Ok(if bad {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn render_report(report: &status::Report) {
    for scope in report.scopes() {
        println!("{}", fmt::bold(&scope));
        // Width is per-section, not global: the demo section's item names
        // are whole relative paths, and letting them set the column for
        // every other section would leave the identity rows swimming.
        let width = report
            .checks
            .iter()
            .filter(|c| c.scope == scope)
            .map(|c| c.item.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(8, 36);
        for check in report.checks.iter().filter(|c| c.scope == scope) {
            println!(
                "  {} {:<width$}  {}",
                check.level.paint(check.level.marker()),
                check.item,
                check.detail,
                width = width
            );
            if let Some(fix) = &check.fix {
                println!(
                    "  {:>width$}    {}",
                    "",
                    fmt::cyan(&format!("→ {fix}")),
                    width = width
                );
            }
        }
        println!();
    }

    let fails = report.count(Level::Fail);
    let warns = report.count(Level::Warn);
    let summary = if fails > 0 {
        fmt::red(&format!(
            "{fails} problem{} to fix before the guide will run, {warns} warning{}",
            plural(fails),
            plural(warns)
        ))
    } else if warns > 0 {
        fmt::yellow(&format!("ready, with {warns} warning{}", plural(warns)))
    } else {
        fmt::green("everything is in order")
    };
    println!("{summary}");
}

// --- save / list / show ----------------------------------------------

fn cmd_save(
    layout: &Layout,
    name: &str,
    selection: &Selection,
    note: Option<String>,
    force: bool,
    json: bool,
) -> Result<ExitCode, String> {
    let slot = slots::save(
        layout,
        name,
        &selection.identities(layout),
        &selection.scopes(),
        note,
        force,
    )?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&slot.manifest).map_err(|e| e.to_string())?
        );
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "saved slot {} — {} file{}, {}",
        fmt::bold(&slot.name),
        slot.manifest.file_count(),
        plural(slot.manifest.file_count()),
        fmt::human_bytes(slot.manifest.total_bytes())
    );
    let ids: Vec<&str> = slot
        .manifest
        .identities
        .iter()
        .map(|i| i.id.as_str())
        .collect();
    if ids.is_empty() {
        println!(
            "  {}",
            fmt::yellow("no identity trees were found to capture — run `demo-env status`")
        );
    } else {
        println!("  identities  {}", ids.join(", "));
    }
    if let Some(endpoint) = &slot.manifest.expected_endpoint {
        println!("  cluster     {endpoint}");
    }
    println!("  at          {}", fmt::tilde(&slot.dir));
    for skipped in &slot.manifest.skipped {
        println!("  {} {skipped}", fmt::yellow("skipped"));
    }
    warn_about_secrets(&slot.dir);
    Ok(ExitCode::SUCCESS)
}

/// A slot holds refresh tokens, a TLS private key and the demo `.env`'s
/// API keys. Saying so once, at save time, is the honest thing to do — the
/// directory mode is already 0700, but the user should know what they now
/// have a copy of.
fn warn_about_secrets(dir: &std::path::Path) {
    println!(
        "  {}",
        fmt::dim(&format!(
            "contains refresh tokens, a TLS private key and .env credentials — {} is mode 0700, keep it out of git",
            fmt::tilde(dir)
        ))
    );
}

fn cmd_list(layout: &Layout, json: bool) -> Result<ExitCode, String> {
    let slots = slots::list(layout);

    if json {
        let payload: Vec<serde_json::Value> = slots
            .iter()
            .map(|slot| match slot {
                Ok(slot) => serde_json::json!({
                    "name": slot.name,
                    "created_at": slot.manifest.created_at,
                    "note": slot.manifest.note,
                    "namespace": slot.manifest.namespace,
                    "expected_endpoint": slot.manifest.expected_endpoint,
                    "identities": slot.manifest.identities.iter().map(|i| &i.id).collect::<Vec<_>>(),
                    "scopes": slot.manifest.scopes,
                    "files": slot.manifest.file_count(),
                    "bytes": slot.manifest.total_bytes(),
                }),
                Err(e) => serde_json::json!({ "error": e }),
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?
        );
        return Ok(ExitCode::SUCCESS);
    }

    if slots.is_empty() {
        println!(
            "no slots yet in {} — create one with `demo-env save <name>`",
            fmt::tilde(&layout.slots_dir)
        );
        return Ok(ExitCode::SUCCESS);
    }

    let name_width = slots
        .iter()
        .filter_map(|s| s.as_ref().ok())
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 28);

    let current = layout.env.expected_endpoint();
    for slot in &slots {
        match slot {
            Ok(slot) => {
                let m = &slot.manifest;
                // Mark the slot that matches the cluster .env points at
                // now: with several saved, that is the one restore is
                // almost always about.
                let marker = match (&current, &m.expected_endpoint) {
                    (Some(a), Some(b)) if a == b => fmt::green("*"),
                    _ => " ".to_string(),
                };
                let ids: Vec<&str> = m.identities.iter().map(|i| i.id.as_str()).collect();
                println!(
                    "{marker} {:<name_width$}  {}  {}",
                    fmt::bold(&slot.name),
                    fmt::local_time(&m.created_at),
                    fmt::dim(&format!(
                        "{} · {} file{} · {}",
                        m.namespace.as_deref().unwrap_or("namespace ?"),
                        m.file_count(),
                        plural(m.file_count()),
                        fmt::human_bytes(m.total_bytes())
                    )),
                    name_width = name_width
                );
                println!(
                    "  {:<name_width$}  {}",
                    "",
                    fmt::dim(&if ids.is_empty() {
                        "no identities".to_string()
                    } else {
                        ids.join(", ")
                    }),
                    name_width = name_width
                );
                if let Some(note) = &m.note {
                    println!("  {:<name_width$}  {note}", "", name_width = name_width);
                }
            }
            Err(e) => println!("{} {}", fmt::red("✗"), e),
        }
    }
    if current.is_some() {
        println!(
            "\n{}",
            fmt::dim("* matches the cluster this checkout's .env points at")
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_show(layout: &Layout, name: &str, json: bool) -> Result<ExitCode, String> {
    let slot = slots::load(layout, name)?;
    let m = &slot.manifest;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(m).map_err(|e| e.to_string())?
        );
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "{}  {}",
        fmt::bold(&slot.name),
        fmt::dim(&fmt::tilde(&slot.dir))
    );
    println!("  saved      {}", fmt::local_time(&m.created_at));
    if let Some(note) = &m.note {
        println!("  note       {note}");
    }
    println!("  scopes     {}", m.scopes.join(", "));
    println!("  gateway    {}", m.gateway);
    println!(
        "  cluster    {}",
        m.expected_endpoint.as_deref().unwrap_or("unknown")
    );
    println!(
        "  contents   {} file{}, {}",
        m.file_count(),
        plural(m.file_count()),
        fmt::human_bytes(m.total_bytes())
    );
    println!("  tool       demo-env {}", m.tool_version);
    println!();

    if m.identities.is_empty() {
        println!("{}", fmt::dim("no identities in this slot"));
    } else {
        println!("{}", fmt::bold("identities"));
        for entry in &m.identities {
            // A slot whose personas disagree about the endpoint was saved
            // mid-migration and will not work as a set — call it out here
            // rather than letting restore produce a mixed state.
            let consistent = match (&m.expected_endpoint, &entry.gateway_endpoint) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            };
            let endpoint = entry.gateway_endpoint.as_deref().unwrap_or("endpoint ?");
            let note = if consistent {
                fmt::dim(endpoint)
            } else {
                fmt::yellow(&format!("{endpoint}  ← different cluster to the rest"))
            };
            println!(
                "  {:<10} {:>4} file{}  {}",
                entry.id,
                entry.files.len(),
                plural(entry.files.len()),
                note
            );
        }
    }
    if !m.demo_artifacts.is_empty() {
        println!("\n{}", fmt::bold("demo files"));
        for rel in &m.demo_artifacts {
            println!("  {rel}");
        }
    }
    if !m.shared.is_empty() {
        println!("\n{}", fmt::bold("shared"));
        for file in &m.shared {
            println!("  {}  {}", file.path, fmt::human_bytes(file.size));
        }
    }
    if !m.skipped.is_empty() {
        println!("\n{}", fmt::yellow("skipped at save time"));
        for skipped in &m.skipped {
            println!("  {skipped}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

// --- restore / diff / rm ---------------------------------------------

fn cmd_restore(
    layout: &Layout,
    name: &str,
    selection: &Selection,
    yes: bool,
    dry_run: bool,
    no_backup: bool,
    json: bool,
) -> Result<ExitCode, String> {
    let slot = slots::load(layout, name)?;
    let scopes = selection.scopes();

    for scope in &scopes {
        if !slot.manifest.has_scope(*scope) {
            eprintln!(
                "{} slot '{name}' was saved without scope '{}' — nothing to restore there",
                fmt::yellow("note:"),
                scope.as_str()
            );
        }
    }

    let steps = slots::plan_restore(layout, &slot, selection.identity_filter(), &scopes)?;
    if steps.is_empty() {
        return Err(format!(
            "slot '{name}' has nothing matching that selection to restore"
        ));
    }

    // The plan is shown before anything is written, in whichever format
    // was asked for — it is the only chance to notice that a slot belongs
    // to a different cluster than the one you meant.
    if json {
        let payload: Vec<serde_json::Value> = steps
            .iter()
            .map(|s| {
                serde_json::json!({
                    "label": s.label,
                    "to": s.to.display().to_string(),
                    "replaces": s.replaces,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?
        );
    } else {
        println!("restoring slot {} will replace:", fmt::bold(&slot.name));
        let width = steps
            .iter()
            .map(|s| s.label.chars().count())
            .max()
            .unwrap_or(0);
        for step in &steps {
            let verb = if step.replaces {
                fmt::yellow("replace")
            } else {
                fmt::green("create ")
            };
            println!(
                "  {verb} {:<width$}  {}",
                step.label,
                fmt::tilde(&step.to),
                width = width
            );
        }
        if let Some(endpoint) = &slot.manifest.expected_endpoint {
            println!("  {}", fmt::dim(&format!("slot's cluster: {endpoint}")));
        }
    }

    if dry_run {
        if !json {
            println!("\n{}", fmt::dim("--dry-run: nothing written"));
        }
        return Ok(ExitCode::SUCCESS);
    }

    if !yes && !confirm("Proceed?")? {
        println!("aborted");
        return Ok(ExitCode::SUCCESS);
    }

    // Autosave first, covering exactly what is about to be overwritten, so
    // a restore onto a working setup is always undoable.
    if !no_backup {
        let backup_name = format!("autosave-{}", fmt::now_slot_stamp());
        match slots::save(
            layout,
            &backup_name,
            &selection.identities(layout),
            &scopes,
            Some(format!("automatic backup taken before restoring '{name}'")),
            false,
        ) {
            Ok(backup) => println!(
                "backed up current state to slot {}",
                fmt::bold(&backup.name)
            ),
            Err(e) => {
                return Err(format!(
                    "could not back up the current state ({e}); re-run with --no-backup to restore anyway"
                ));
            }
        }
    }

    slots::apply_restore(&steps)?;
    println!("restored slot {}", fmt::bold(&slot.name));
    println!(
        "  {}",
        fmt::dim(
            "already-open terminals keep their old environment — re-run `source scripts/as.sh <id>` in each"
        )
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_diff(
    layout: &Layout,
    name: &str,
    selection: &Selection,
    all: bool,
    json: bool,
) -> Result<ExitCode, String> {
    let slot = slots::load(layout, name)?;
    let entries = slots::diff(
        layout,
        &slot,
        selection.identity_filter(),
        &selection.scopes(),
    );

    let shown: Vec<&slots::DiffEntry> = entries
        .iter()
        .filter(|e| all || e.state != slots::DiffState::Same)
        .collect();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&shown).map_err(|e| e.to_string())?
        );
    } else if shown.is_empty() {
        println!("slot {} matches the files on disk", fmt::bold(&slot.name));
    } else {
        for entry in &shown {
            let paint: fn(&str) -> String = match entry.state {
                slots::DiffState::Same => fmt::dim,
                slots::DiffState::Changed => fmt::yellow,
                slots::DiffState::OnlyInSlot => fmt::red,
                slots::DiffState::OnlyLive => fmt::green,
            };
            println!("{} {}", paint(entry.state.marker()), entry.path);
        }
        println!(
            "\n{}",
            fmt::dim("~ differs · - only in the slot · + only on disk")
        );
    }

    // Non-zero when the slot and the live state have diverged, so this is
    // usable as a guard in a script.
    Ok(if shown.iter().any(|e| e.state != slots::DiffState::Same) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn cmd_rm(layout: &Layout, name: &str, yes: bool) -> Result<ExitCode, String> {
    let slot = slots::load(layout, name)?;
    println!(
        "slot {} — saved {}, {} file{}",
        fmt::bold(&slot.name),
        fmt::local_time(&slot.manifest.created_at),
        slot.manifest.file_count(),
        plural(slot.manifest.file_count())
    );
    if !yes && !confirm(&format!("Delete {}?", fmt::tilde(&slot.dir)))? {
        println!("aborted");
        return Ok(ExitCode::SUCCESS);
    }
    slots::remove(layout, name)?;
    println!("deleted slot {name}");
    Ok(ExitCode::SUCCESS)
}

/// Ask before doing something irreversible. Refuses to assume yes when
/// stdin isn't a terminal — a piped or CI invocation has to say `--yes`
/// explicitly rather than have it inferred.
fn confirm(question: &str) -> Result<bool, String> {
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{question} — stdin is not a terminal; pass --yes to confirm non-interactively"
        ));
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
