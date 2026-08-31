//! CLI command handlers.
//!
//! Each function implements one subcommand: `init`, `checkpoint`, `list`,
//! `diff`, `restore`, `gc`. Both human-readable and `--json` output paths
//! are implemented in each handler.

use crate::cli::format::{absolutize, format_timestamp, now_unix};
use crate::core::CheckpointMeta;
use crate::error::{Result, VarnError};
use crate::filesystem::Scanner;
use crate::platform;
use crate::restore;
use crate::snapshot::SnapshotData;
use crate::storage::Repo;
use crate::storage::git_guard;
use std::io::{self, Write};
use std::path::PathBuf;

/// `varn init`
pub fn cmd_init(path: &PathBuf, gitignore: bool, json: bool) -> Result<()> {
    let abs = absolutize(path)?;
    let repo = Repo::init(&abs, platform::os_name())?;

    // Optional: add `.varn/` to the enclosing repository's root .gitignore.
    // The store-level guard (`.varn/.gitignore`) already covers this for Git;
    // this flag is for users who prefer a root-level entry.
    let mut warnings: Vec<String> = Vec::new();
    let gitignore_result = if gitignore {
        let git_root = git_guard::find_git_root(&repo.root).ok_or_else(|| {
            VarnError::Other(
                "--gitignore requested but no git repository found \
                 (searched upward from the init path)"
                    .to_string(),
            )
        })?;
        Some(git_guard::append_to_gitignore(&git_root)?)
    } else {
        None
    };

    // Advisory: warn if the store is not excluded from git. With the
    // store-level guard this should not fire, but legacy stores created
    // before the guard may lack it.
    if let Some(warning) = git_guard::coexistence_warning(&repo.root, &repo.varn_dir) {
        warnings.push(warning.to_string());
    }

    if json {
        let mut output = serde_json::json!({
            "status": "ok",
            "root": repo.root,
            "varn_dir": repo.varn_dir,
            "version": repo.config.version,
            "platform": repo.config.platform,
            "warnings": warnings,
        });
        if let Some(update) = gitignore_result {
            output["gitignore"] = match update {
                git_guard::GitignoreUpdate::Created => serde_json::json!("created"),
                git_guard::GitignoreUpdate::Appended => serde_json::json!("appended"),
                git_guard::GitignoreUpdate::AlreadyPresent => serde_json::json!("already_present"),
            };
        }
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Initialized Varn repository at {}", repo.root.display());
        println!("  storage version: {}", repo.config.version);
        println!("  platform: {}", repo.config.platform);
        if let Some(update) = gitignore_result {
            let verb = match update {
                git_guard::GitignoreUpdate::Created => "created",
                git_guard::GitignoreUpdate::Appended => "added .varn/ to",
                git_guard::GitignoreUpdate::AlreadyPresent => "already ignored in",
            };
            let git_root = git_guard::find_git_root(&repo.root);
            if let Some(git_root) = git_root {
                println!(
                    "  gitignore: {verb} {}",
                    git_root.join(".gitignore").display()
                );
            }
        }
        for w in &warnings {
            println!("  warning: {w}");
        }
    }
    Ok(())
}

/// `varn checkpoint`
pub fn cmd_checkpoint(description: &str, json: bool) -> Result<()> {
    let repo = Repo::open(&PathBuf::from("."))?;

    // Load the scan cache for incremental scanning.
    let cache_path = repo.varn_dir.join("index").join("scan_cache.json");
    let cache = crate::filesystem::ScanCache::load(&cache_path);

    // Scan the filesystem.
    let mut scanner = Scanner::with_ignore(&repo.root);
    scanner.set_cache(cache);
    let scan_result = scanner.scan()?;

    // Save the updated cache for the next scan.
    scan_result.cache.save(&cache_path)?;

    // Build the snapshot data (generates the checkpoint ID).
    let meta = CheckpointMeta {
        id: crate::core::CheckpointId("pending".to_string()),
        description: description.to_string(),
        created_at: now_unix(),
        root: repo.root.clone(),
    };
    let snapshot = SnapshotData::new(meta, scan_result.entries);

    // Store file content blobs in the object store.
    snapshot.store_content_blobs(&repo.root, &repo.object_store())?;

    // Persist the snapshot (idempotent: no-op if an identical one exists).
    let saved = snapshot.save(&repo.snapshots_dir())?;

    // Advisory: warn if the store is not excluded from git (legacy stores
    // created before the store-level guard existed).
    let git_warning = git_guard::coexistence_warning(&repo.root, &repo.varn_dir);

    // Report any scan warnings.
    if json {
        let mut warnings: Vec<serde_json::Value> = scan_result
            .warnings
            .iter()
            .map(|w| {
                serde_json::json!({
                    "path": w.path,
                    "message": w.message,
                })
            })
            .collect::<Vec<_>>();
        if let Some(w) = git_warning {
            warnings.push(serde_json::json!({
                "path": null,
                "message": w,
            }));
        }
        let output = serde_json::json!({
            "status": if saved { "ok" } else { "unchanged" },
            "checkpoint_id": snapshot.meta.id.0,
            "description": snapshot.meta.description,
            "created_at": snapshot.meta.created_at,
            "root": snapshot.meta.root,
            "entries": snapshot.entries.len(),
            "saved": saved,
            "warnings": warnings,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        if saved {
            println!(
                "Checkpoint {} created: {}",
                snapshot.meta.id.0, snapshot.meta.description
            );
        } else {
            println!(
                "Checkpoint {} already exists (no changes): {}",
                snapshot.meta.id.0, snapshot.meta.description
            );
        }
        println!("  entries: {}", snapshot.entries.len());
        if !scan_result.warnings.is_empty() {
            println!("  warnings: {}", scan_result.warnings.len());
            for w in &scan_result.warnings {
                println!("    {}: {}", w.path.display(), w.message);
            }
        }
        if let Some(w) = git_warning {
            println!("  warning: {w}");
        }
    }
    Ok(())
}

/// `varn list`
pub fn cmd_list(json: bool) -> Result<()> {
    let repo = Repo::open(&PathBuf::from("."))?;
    let snapshots = SnapshotData::list_all(&repo.snapshots_dir())?;

    if json {
        let output: Vec<_> = snapshots
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.meta.id.0,
                    "description": s.meta.description,
                    "created_at": s.meta.created_at,
                    "entries": s.entries.len(),
                })
            })
            .collect();
        let result = serde_json::json!({
            "status": "ok",
            "checkpoints": output,
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if snapshots.is_empty() {
        println!("No checkpoints found.");
    } else {
        println!("{:<14} {:<20} DESCRIPTION", "ID", "TIME");
        for s in &snapshots {
            let time = format_timestamp(s.meta.created_at);
            println!("{:<14} {:<20} {}", s.meta.id.0, time, s.meta.description);
        }
    }
    Ok(())
}

/// `varn diff`
pub fn cmd_diff(checkpoint: &str, json: bool) -> Result<()> {
    let repo = Repo::open(&PathBuf::from("."))?;

    // Load the target snapshot.
    let snapshot = resolve_checkpoint(&repo, checkpoint)?;

    // Scan the current filesystem state.
    let scanner = Scanner::with_ignore(&repo.root);
    let current = scanner.scan()?;

    // Compute the diff.
    let changes = crate::diff::diff_states(&snapshot.entries, &current.entries);

    if json {
        let output = serde_json::json!({
            "status": "ok",
            "checkpoint": snapshot.meta.id.0,
            "changes": changes.iter().map(|c| {
                serde_json::json!({
                    "kind": match c.kind {
                        crate::diff::ChangeKind::Added => "added",
                        crate::diff::ChangeKind::Modified => "modified",
                        crate::diff::ChangeKind::Removed => "removed",
                    },
                    "path": c.path,
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if changes.is_empty() {
        println!("No changes detected.");
    } else {
        let added: Vec<_> = changes
            .iter()
            .filter(|c| c.kind == crate::diff::ChangeKind::Added)
            .collect();
        let modified: Vec<_> = changes
            .iter()
            .filter(|c| c.kind == crate::diff::ChangeKind::Modified)
            .collect();
        let removed: Vec<_> = changes
            .iter()
            .filter(|c| c.kind == crate::diff::ChangeKind::Removed)
            .collect();

        if !added.is_empty() {
            println!("ADDED");
            for c in &added {
                println!("  {}", c.path.display());
            }
        }
        if !modified.is_empty() {
            println!("MODIFIED");
            for c in &modified {
                println!("  {}", c.path.display());
            }
        }
        if !removed.is_empty() {
            println!("DELETED");
            for c in &removed {
                println!("  {}", c.path.display());
            }
        }
    }
    Ok(())
}

/// `varn restore`
pub fn cmd_restore(checkpoint: &str, yes: bool, no_safety: bool, json: bool) -> Result<()> {
    let repo = Repo::open(&PathBuf::from("."))?;

    // Load the target snapshot.
    let snapshot = resolve_checkpoint(&repo, checkpoint)?;

    // Scan the current filesystem state.
    let scanner = Scanner::with_ignore(&repo.root);
    let current = scanner.scan()?;

    // Plan the restore.
    let plan = restore::plan_restore(&snapshot.entries, &current.entries);

    // Check for conflicts and confirm.
    if plan.has_conflicts() && !yes {
        if json {
            // In JSON mode with conflicts and no --yes, report and exit.
            let output = serde_json::json!({
                "status": "conflicts",
                "checkpoint": snapshot.meta.id.0,
                "conflicts": plan.conflicts.iter().map(|c| {
                    let (kind, path) = match c {
                        restore::Conflict::Modified { path } => ("modified", path),
                        restore::Conflict::Unexpected { path } => ("unexpected", path),
                    };
                    serde_json::json!({
                        "kind": kind,
                        "path": path,
                    })
                }).collect::<Vec<_>>(),
                "actions": plan.action_count(),
                "message": "Conflicts detected. Re-run with --yes to proceed.",
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        } else {
            // Interactive confirmation.
            println!("Restore of checkpoint {} would:", snapshot.meta.id.0);
            for c in &plan.conflicts {
                match c {
                    restore::Conflict::Modified { path } => {
                        println!("  OVERWRITE  {}", path.display());
                    }
                    restore::Conflict::Unexpected { path } => {
                        println!("  DELETE     {}", path.display());
                    }
                }
            }
            println!();
            print!("Proceed? [y/N] ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();
            if input != "y" && input != "yes" {
                println!("Restore cancelled. No changes were made.");
                return Ok(());
            }
        }
    }

    // Create a safety checkpoint of the current state before restoring.
    // This allows the user to undo a failed or unwanted restore.
    let safety_id = if !no_safety {
        let safety_meta = CheckpointMeta {
            id: crate::core::CheckpointId("pending".to_string()),
            description: format!(
                "[safety before restore of {}] {}",
                snapshot.meta.id.0, snapshot.meta.description
            ),
            created_at: now_unix(),
            root: repo.root.clone(),
        };
        let safety_snapshot = SnapshotData::new(safety_meta, current.entries.clone());
        // Store content blobs so the safety checkpoint is restorable.
        safety_snapshot.store_content_blobs(&repo.root, &repo.object_store())?;
        let saved = safety_snapshot.save(&repo.snapshots_dir())?;
        let id = safety_snapshot.meta.id.0.clone();
        if saved {
            if !json {
                println!("Safety checkpoint {} created before restore.", id);
            }
        } else if !json {
            println!("Safety checkpoint {} already exists.", id);
        }
        Some(id)
    } else {
        None
    };

    // Execute the restore.
    let mut result = restore::execute_restore(&plan, &repo.root, &repo.object_store())?;

    // Merge plan warnings into the result.
    result.warnings.extend(plan.warnings.clone());

    // Verify the restore.
    result.verified = restore::verify_restore(&repo.root, &snapshot.entries);

    if json {
        let output = serde_json::json!({
            "status": if result.verified { "ok" } else { "verification_failed" },
            "checkpoint": snapshot.meta.id.0,
            "safety_checkpoint": safety_id,
            "files_written": result.files_written,
            "dirs_created": result.dirs_created,
            "symlinks_created": result.symlinks_created,
            "deleted": result.deleted,
            "verified": result.verified,
            "warnings": result.warnings,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Restored checkpoint {}", snapshot.meta.id.0);
        println!("  files written: {}", result.files_written);
        println!("  directories created: {}", result.dirs_created);
        println!("  symlinks created: {}", result.symlinks_created);
        println!("  deleted: {}", result.deleted);
        if result.verified {
            println!("  verification: passed");
        } else {
            println!("  verification: FAILED");
        }
        if let Some(ref sid) = safety_id {
            if !result.verified {
                println!("  safety checkpoint {} can be used to recover", sid);
            }
        }
        if !result.warnings.is_empty() {
            println!("  warnings:");
            for w in &result.warnings {
                println!("    {w}");
            }
        }
    }

    if !result.verified {
        return Err(VarnError::Other(format!(
            "restore verification failed: filesystem state does not match checkpoint{}",
            safety_id
                .as_ref()
                .map(|id| format!("; safety checkpoint {id} available for recovery"))
                .unwrap_or_default()
        )));
    }

    Ok(())
}

/// `varn gc`
pub fn cmd_gc(dry_run: bool, json: bool) -> Result<()> {
    let repo = Repo::open(&PathBuf::from("."))?;
    let result = crate::storage::garbage_collect(&repo, dry_run)?;

    if json {
        let output = serde_json::json!({
            "status": "ok",
            "dry_run": dry_run,
            "total_objects": result.total_objects,
            "referenced_objects": result.referenced_objects,
            "deleted": result.deleted,
            "deleted_hashes": result.deleted_hashes,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        if dry_run {
            println!("Garbage collection (dry run):");
        } else {
            println!("Garbage collection complete:");
        }
        println!("  total objects: {}", result.total_objects);
        println!("  referenced: {}", result.referenced_objects);
        println!(
            "  {}: {}",
            if dry_run { "would delete" } else { "deleted" },
            result.deleted
        );
        if !result.deleted_hashes.is_empty() && !dry_run {
            println!("  deleted objects:");
            for hash in &result.deleted_hashes {
                println!("    {hash}");
            }
        }
    }
    Ok(())
}

/// `varn migrate`
pub fn cmd_migrate(dry_run: bool, json: bool) -> Result<()> {
    let repo = Repo::open(&PathBuf::from("."))?;

    let needs = crate::storage::needs_migration(&repo);

    // Backfill the store-level git guard for stores created before it
    // existed. Safe to run on every migrate: it never overwrites an
    // existing guard file and never touches anything outside `.varn/`.
    let guard_added = if dry_run {
        !git_guard::guard_present(&repo.varn_dir)
    } else {
        git_guard::ensure_guard(&repo.varn_dir)?
    };

    if json {
        let output = serde_json::json!({
            "status": if needs { "needs_migration" } else { "ok" },
            "current_version": repo.config.version,
            "target_version": crate::storage::STORAGE_VERSION,
            "needs_migration": needs,
            "dry_run": dry_run,
            "git_guard": if guard_added { "added" } else { "present" },
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        if needs {
            println!(
                "Repository version {} needs migration to version {}",
                repo.config.version,
                crate::storage::STORAGE_VERSION
            );
            if dry_run {
                println!("  (dry run — no changes made)");
            } else {
                crate::storage::migrate_repo(&repo)?;
                println!("Migration complete.");
            }
        } else {
            println!(
                "Repository is already at version {} (current).",
                repo.config.version
            );
        }
        if guard_added {
            println!("  git guard: added .varn/.gitignore (git now ignores the store)");
        }
    }
    Ok(())
}

/// Resolve a checkpoint ID (full or prefix) to a [`SnapshotData`].
///
/// If the ID is a prefix that matches exactly one checkpoint, it is
/// resolved. If it matches multiple, an error is returned.
pub fn resolve_checkpoint(repo: &Repo, id_or_prefix: &str) -> Result<SnapshotData> {
    let snapshots = SnapshotData::list_all(&repo.snapshots_dir())?;

    // Try exact match first.
    for s in &snapshots {
        if s.meta.id.0 == id_or_prefix {
            return Ok(s.clone());
        }
    }

    // Try prefix match.
    let matches: Vec<_> = snapshots
        .iter()
        .filter(|s| s.meta.id.0.starts_with(id_or_prefix))
        .collect();

    match matches.len() {
        0 => Err(VarnError::Other(format!(
            "checkpoint not found: {id_or_prefix}"
        ))),
        1 => Ok(matches[0].clone()),
        _ => Err(VarnError::Other(format!(
            "ambiguous checkpoint prefix '{id_or_prefix}' matches {} checkpoints",
            matches.len()
        ))),
    }
}
