//! CLI layer: argument parsing, output formatting, and exit codes.
//!
//! The CLI is designed so that another program can reliably consume its
//! output. When `--json` is passed, commands emit structured JSON to stdout
//! and errors are emitted as JSON to stderr.

use crate::core::CheckpointMeta;
use crate::error::{Result, VarnError};
use crate::filesystem::Scanner;
use crate::platform;
use crate::restore;
use crate::snapshot::SnapshotData;
use crate::storage::Repo;
use clap::{Parser, Subcommand};
use std::io::{self, Write};
use std::path::PathBuf;

/// Varn — local state checkpointing and rollback.
#[derive(Parser, Debug)]
#[command(name = "varn", version, about, long_about = None)]
pub struct Cli {
    /// Emit machine-readable JSON output.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Initialize Varn metadata for a directory.
    Init {
        /// The directory to initialize. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Capture the current filesystem state.
    Checkpoint {
        /// A human-readable description of this checkpoint.
        description: String,
    },
    /// Display available checkpoints.
    List,
    /// Compare the current state with a checkpoint.
    Diff {
        /// The checkpoint to compare against (id or prefix).
        checkpoint: String,
    },
    /// Restore a checkpoint.
    Restore {
        /// The checkpoint to restore (id or prefix).
        checkpoint: String,
        /// Skip confirmation prompts (use with care).
        #[arg(long)]
        yes: bool,
    },
}

/// Run a parsed CLI invocation.
pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init { path } => cmd_init(&path, cli.json),
        Command::Checkpoint { description } => cmd_checkpoint(&description, cli.json),
        Command::List => cmd_list(cli.json),
        Command::Diff { checkpoint } => cmd_diff(&checkpoint, cli.json),
        Command::Restore { checkpoint, yes } => cmd_restore(&checkpoint, yes, cli.json),
    }
}

/// `varn init`
fn cmd_init(path: &PathBuf, json: bool) -> Result<()> {
    let abs = absolutize(path)?;
    let repo = Repo::init(&abs, platform::os_name())?;
    if json {
        let output = serde_json::json!({
            "status": "ok",
            "root": repo.root,
            "varn_dir": repo.varn_dir,
            "version": repo.config.version,
            "platform": repo.config.platform,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Initialized Varn repository at {}", repo.root.display());
        println!("  storage version: {}", repo.config.version);
        println!("  platform: {}", repo.config.platform);
    }
    Ok(())
}

/// `varn checkpoint`
fn cmd_checkpoint(description: &str, json: bool) -> Result<()> {
    let repo = Repo::open(&PathBuf::from("."))?;

    // Scan the filesystem.
    let scanner = Scanner::new(&repo.root);
    let scan_result = scanner.scan()?;

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

    // Persist the snapshot.
    snapshot.save(&repo.snapshots_dir())?;

    // Report any scan warnings.
    if json {
        let output = serde_json::json!({
            "status": "ok",
            "checkpoint_id": snapshot.meta.id.0,
            "description": snapshot.meta.description,
            "created_at": snapshot.meta.created_at,
            "root": snapshot.meta.root,
            "entries": snapshot.entries.len(),
            "warnings": scan_result.warnings.iter().map(|w| {
                serde_json::json!({
                    "path": w.path,
                    "message": w.message,
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!(
            "Checkpoint {} created: {}",
            snapshot.meta.id.0, snapshot.meta.description
        );
        println!("  entries: {}", snapshot.entries.len());
        if !scan_result.warnings.is_empty() {
            println!("  warnings: {}", scan_result.warnings.len());
            for w in &scan_result.warnings {
                println!("    {}: {}", w.path.display(), w.message);
            }
        }
    }
    Ok(())
}

/// `varn list`
fn cmd_list(json: bool) -> Result<()> {
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
fn cmd_diff(checkpoint: &str, json: bool) -> Result<()> {
    let repo = Repo::open(&PathBuf::from("."))?;

    // Load the target snapshot.
    let snapshot = resolve_checkpoint(&repo, checkpoint)?;

    // Scan the current filesystem state.
    let scanner = Scanner::new(&repo.root);
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
fn cmd_restore(checkpoint: &str, yes: bool, json: bool) -> Result<()> {
    let repo = Repo::open(&PathBuf::from("."))?;

    // Load the target snapshot.
    let snapshot = resolve_checkpoint(&repo, checkpoint)?;

    // Scan the current filesystem state.
    let scanner = Scanner::new(&repo.root);
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

    // Execute the restore.
    let mut result = restore::execute_restore(&plan, &repo.root, &repo.object_store())?;

    // Verify the restore.
    result.verified = restore::verify_restore(&repo.root, &snapshot.entries);

    if json {
        let output = serde_json::json!({
            "status": if result.verified { "ok" } else { "verification_failed" },
            "checkpoint": snapshot.meta.id.0,
            "files_written": result.files_written,
            "dirs_created": result.dirs_created,
            "deleted": result.deleted,
            "verified": result.verified,
            "warnings": result.warnings,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Restored checkpoint {}", snapshot.meta.id.0);
        println!("  files written: {}", result.files_written);
        println!("  directories created: {}", result.dirs_created);
        println!("  deleted: {}", result.deleted);
        if result.verified {
            println!("  verification: passed");
        } else {
            println!("  verification: FAILED");
        }
        if !result.warnings.is_empty() {
            println!("  warnings:");
            for w in &result.warnings {
                println!("    {w}");
            }
        }
    }

    if !result.verified {
        return Err(VarnError::Other(
            "restore verification failed: filesystem state does not match checkpoint".to_string(),
        ));
    }

    Ok(())
}

/// Resolve a checkpoint ID (full or prefix) to a [`SnapshotData`].
///
/// If the ID is a prefix that matches exactly one checkpoint, it is
/// resolved. If it matches multiple, an error is returned.
fn resolve_checkpoint(repo: &Repo, id_or_prefix: &str) -> Result<SnapshotData> {
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

/// Resolve a possibly-relative path to an absolute one without following
/// symlinks in the final component.
fn absolutize(path: &PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.clone());
    }
    let cwd = std::env::current_dir()
        .map_err(|e| VarnError::Other(format!("could not determine current directory: {e}")))?;
    Ok(cwd.join(path))
}

/// Format a UNIX timestamp as `YYYY-MM-DD HH:MM` (UTC).
fn format_timestamp(ts: i64) -> String {
    // Simple formatting without external dependencies.
    // Computes UTC date/time without a timezone library.
    let secs = ts as u64;
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;

    // Compute date from days since 1970-01-01.
    let (year, month, day) = days_to_date(days_since_epoch);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Convert days since 1970-01-01 to (year, month, day).
/// Based on the Howard Hinnant date algorithm.
fn days_to_date(days: u64) -> (u32, u32, u32) {
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year as u32, m as u32, d as u32)
}

/// Current time as seconds since the UNIX epoch.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_init() {
        let cli = Cli::try_parse_from(["varn", "init", "/tmp/foo"]).unwrap();
        assert!(!cli.json);
        assert!(matches!(cli.command, Command::Init { .. }));
        if let Command::Init { path } = cli.command {
            assert_eq!(path, PathBuf::from("/tmp/foo"));
        }
    }

    #[test]
    fn cli_parses_checkpoint_with_description() {
        let cli = Cli::try_parse_from(["varn", "checkpoint", "my desc"]).unwrap();
        if let Command::Checkpoint { description } = cli.command {
            assert_eq!(description, "my desc");
        } else {
            panic!("wrong command");
        }
    }

    #[test]
    fn cli_parses_restore_with_yes() {
        let cli = Cli::try_parse_from(["varn", "restore", "abc", "--yes"]).unwrap();
        if let Command::Restore { checkpoint, yes } = cli.command {
            assert_eq!(checkpoint, "abc");
            assert!(yes);
        } else {
            panic!("wrong command");
        }
    }

    #[test]
    fn cli_parses_json_global_flag() {
        let cli = Cli::try_parse_from(["varn", "--json", "list"]).unwrap();
        assert!(cli.json);
        assert!(matches!(cli.command, Command::List));
    }

    #[test]
    fn cli_parses_init_default_path() {
        let cli = Cli::try_parse_from(["varn", "init"]).unwrap();
        if let Command::Init { path } = cli.command {
            assert_eq!(path, PathBuf::from("."));
        } else {
            panic!("wrong command");
        }
    }

    #[test]
    fn format_timestamp_known_value() {
        // 2026-08-19 20:14 in UTC (timestamp 1787162040)
        // Note: this is UTC; local time may differ.
        let ts = 1787162040;
        let formatted = format_timestamp(ts);
        assert!(formatted.starts_with("2026-08-19"));
    }

    #[test]
    fn days_to_date_epoch() {
        // 1970-01-01 is day 0.
        let (y, m, d) = days_to_date(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_date_known() {
        // 2026-08-19 is day 20684 since epoch.
        let (y, m, d) = days_to_date(20684);
        assert_eq!((y, m, d), (2026, 8, 19));
    }
}
