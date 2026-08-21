//! CLI argument parsing and command dispatch.
//!
//! The CLI is designed so that another program can reliably consume its
//! output. When `--json` is passed, commands emit structured JSON to stdout
//! and errors are emitted as JSON to stderr.

use crate::error::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod commands;
pub mod format;

// Re-export command functions and helpers for main.rs.
pub use commands::{
    cmd_checkpoint, cmd_diff, cmd_gc, cmd_init, cmd_list, cmd_restore, resolve_checkpoint,
};
pub use format::{absolutize, format_timestamp, now_unix};

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
        /// Skip creating a safety checkpoint before restore.
        ///
        /// By default, Varn creates a checkpoint of the current state
        /// before restoring, so a failed restore can be undone. Use this
        /// flag to skip that safety measure.
        #[arg(long)]
        no_safety: bool,
    },
    /// Remove objects from the store that no snapshot references.
    Gc {
        /// Show what would be deleted without actually deleting.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Run a parsed CLI invocation.
pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Init { path } => cmd_init(&path, cli.json),
        Command::Checkpoint { description } => cmd_checkpoint(&description, cli.json),
        Command::List => cmd_list(cli.json),
        Command::Diff { checkpoint } => cmd_diff(&checkpoint, cli.json),
        Command::Restore {
            checkpoint,
            yes,
            no_safety,
        } => cmd_restore(&checkpoint, yes, no_safety, cli.json),
        Command::Gc { dry_run } => cmd_gc(dry_run, cli.json),
    }
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
        if let Command::Restore {
            checkpoint,
            yes,
            no_safety,
        } = cli.command
        {
            assert_eq!(checkpoint, "abc");
            assert!(yes);
            assert!(!no_safety);
        } else {
            panic!("wrong command");
        }
    }

    #[test]
    fn cli_parses_restore_with_no_safety() {
        let cli = Cli::try_parse_from(["varn", "restore", "abc", "--yes", "--no-safety"]).unwrap();
        if let Command::Restore {
            checkpoint,
            yes,
            no_safety,
        } = cli.command
        {
            assert_eq!(checkpoint, "abc");
            assert!(yes);
            assert!(no_safety);
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
    fn cli_parses_gc() {
        let cli = Cli::try_parse_from(["varn", "gc"]).unwrap();
        if let Command::Gc { dry_run } = cli.command {
            assert!(!dry_run);
        } else {
            panic!("wrong command");
        }
    }

    #[test]
    fn cli_parses_gc_dry_run() {
        let cli = Cli::try_parse_from(["varn", "gc", "--dry-run"]).unwrap();
        if let Command::Gc { dry_run } = cli.command {
            assert!(dry_run);
        } else {
            panic!("wrong command");
        }
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
