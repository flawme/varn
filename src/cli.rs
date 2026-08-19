//! CLI layer: argument parsing, output formatting, and exit codes.
//!
//! The CLI is designed so that another program can reliably consume its
//! output. When `--json` is passed, commands emit structured JSON to stdout
//! and errors are emitted as JSON to stderr.

use crate::error::{Result, VarnError};
use crate::platform;
use crate::storage::Repo;
use clap::{Parser, Subcommand};
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
        Command::Checkpoint { description } => {
            let _repo = Repo::open(&PathBuf::from("."))?;
            cmd_checkpoint(&description, cli.json)
        }
        Command::List => {
            let _repo = Repo::open(&PathBuf::from("."))?;
            cmd_list(cli.json)
        }
        Command::Diff { checkpoint } => {
            let _repo = Repo::open(&PathBuf::from("."))?;
            cmd_diff(&checkpoint, cli.json)
        }
        Command::Restore { checkpoint, yes } => {
            let _repo = Repo::open(&PathBuf::from("."))?;
            cmd_restore(&checkpoint, yes, cli.json)
        }
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

/// `varn checkpoint` — not yet implemented.
fn cmd_checkpoint(description: &str, json: bool) -> Result<()> {
    emit_not_implemented("checkpoint", json, Some(description))
}

/// `varn list` — not yet implemented.
fn cmd_list(json: bool) -> Result<()> {
    emit_not_implemented("list", json, None)
}

/// `varn diff` — not yet implemented.
fn cmd_diff(checkpoint: &str, json: bool) -> Result<()> {
    emit_not_implemented("diff", json, Some(checkpoint))
}

/// `varn restore` — not yet implemented.
fn cmd_restore(checkpoint: &str, _yes: bool, json: bool) -> Result<()> {
    emit_not_implemented("restore", json, Some(checkpoint))
}

/// Emit a consistent "not implemented" message for commands that are
/// recognized but not yet functional.
fn emit_not_implemented(command: &'static str, json: bool, detail: Option<&str>) -> Result<()> {
    let msg = match detail {
        Some(d) => format!("{command} ({d}) is not yet implemented in this version"),
        None => format!("{command} is not yet implemented in this version"),
    };
    if json {
        let output = serde_json::json!({
            "status": "not_implemented",
            "command": command,
            "message": msg,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{msg}");
    }
    Ok(())
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
}
