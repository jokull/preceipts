use clap::{Parser, Subcommand, ValueEnum};
use std::str::FromStr;

use crate::commit_reference::CommitReference;

/// VCS backend override option
#[derive(Copy, Clone, PartialEq, Eq, ValueEnum, Debug)]
pub enum VcsOverride {
    /// Use git backend
    Git,
    /// Use jj (Jujutsu) backend
    Jj,
}

#[derive(Parser)]
#[command(name = "preceipts")]
#[command(about = "preceipts — the coding-agent PR companion: diff cockpit + local CI receipts", long_about = None)]
#[command(
    after_help = "Engine subcommands (delegated to preceipts-engine):\n  init, run, status, log, land, hud, sync, gc\n\nBare `preceipts` opens the cockpit: the PR diff (merge-base of origin/main\nvs the working tree), watching for changes. Keys: / search, R receipts rail,\nt toggle PR/uncommitted scope, ? all keybindings."
)]
#[command(version)]
pub struct Cli {
    /// Path to configuration file eg: ./path/to/lumen.config.json
    #[arg(long, hide = true)]
    pub config: Option<String>,

    #[arg(value_enum, short = 'p', long = "provider", hide = true)]
    pub provider: Option<ProviderType>,

    #[arg(short = 'k', long = "api-key", hide = true)]
    pub api_key: Option<String>,

    #[arg(short = 'm', long = "model", hide = true)]
    pub model: Option<String>,

    /// Version control system to use (auto-detected if not specified)
    #[arg(value_enum, long = "vcs", hide = true)]
    pub vcs: Option<VcsOverride>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum, Debug)]
pub enum ProviderType {
    Openai,
    Groq,
    Claude,
    Ollama,
    OpencodeZen,
    Openrouter,
    Deepseek,
    Gemini,
    Xai,
    Vercel,
}

impl FromStr for ProviderType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(ProviderType::Openai),
            "groq" => Ok(ProviderType::Groq),
            "claude" => Ok(ProviderType::Claude),
            "ollama" => Ok(ProviderType::Ollama),
            "opencode-zen" => Ok(ProviderType::OpencodeZen),
            "openrouter" => Ok(ProviderType::Openrouter),
            "deepseek" => Ok(ProviderType::Deepseek),
            "gemini" => Ok(ProviderType::Gemini),
            "xai" => Ok(ProviderType::Xai),
            "vercel" => Ok(ProviderType::Vercel),
            _ => Err(format!("Unknown provider: {}", s)),
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Explain the changes in a commit, or the current diff (default). Use --list to select commit interactively
    #[command(hide = true)]
    Explain {
        /// Commit reference: SHA, HEAD, HEAD~3..HEAD, main..feature, main...feature, main..- (range + working tree)
        #[arg(value_parser = clap::value_parser!(CommitReference))]
        reference: Option<CommitReference>,

        /// Use staged diff only (when showing uncommitted changes)
        #[arg(long)]
        staged: bool,

        /// Ask a question instead of summary
        #[arg(short, long)]
        query: Option<String>,

        /// Select commit interactively using fuzzy finder
        #[arg(long)]
        list: bool,
    },
    /// List all commits in an interactive fuzzy-finder, and summarize the changes
    #[command(hide = true)]
    List,
    /// Generate a commit message for the staged changes
    #[command(hide = true)]
    Draft {
        /// Add context to communicate intent
        #[arg(short, long)]
        context: Option<String>,
    },

    #[command(hide = true)]
    Operate {
        #[arg()]
        query: String,
    },
    /// Open the diff cockpit (this is the default when run with no arguments)
    Diff {
        /// Commit reference: SHA, HEAD, HEAD~3..HEAD, main..feature, main...feature
        /// Can also be a PR number or URL (e.g., 123 or https://github.com/owner/repo/pull/123)
        #[arg(value_parser = clap::value_parser!(CommitReference))]
        reference: Option<CommitReference>,

        /// View a GitHub pull request (number or URL)
        #[arg(long)]
        pr: Option<String>,

        /// Detect the PR associated with the current branch and view it
        #[arg(long = "detect-pr", conflicts_with = "pr")]
        detect_pr: bool,

        /// Filter to specific files
        #[arg(short, long)]
        file: Option<Vec<String>>,

        /// Watch for file changes and auto-reload (default: on)
        #[arg(short, long, hide = true)]
        watch: bool,

        /// Disable watch mode
        #[arg(long = "no-watch")]
        no_watch: bool,

        /// Color theme (e.g., dracula, nord, gruvbox-dark, catppuccin-mocha)
        #[arg(short, long)]
        theme: Option<String>,

        /// Show commits stacked (commit-by-commit navigation with ctrl+l/h)
        #[arg(long)]
        stacked: bool,

        /// Initially focus on this file path
        #[arg(long)]
        focus: Option<String>,

        /// Origin repository in owner/repo format (default: origin git remote)
        #[arg(long)]
        origin: Option<String>,

        /// Soft-wrap long diff lines instead of scrolling horizontally
        #[arg(long)]
        wrap: bool,
    },
    /// Interactively configure Lumen (provider, API key)
    #[command(hide = true)]
    Configure,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vcs_git_parses() {
        let cli = Cli::try_parse_from(["lumen", "--vcs", "git", "diff"]).unwrap();
        assert_eq!(cli.vcs, Some(VcsOverride::Git));
    }

    #[test]
    fn test_vcs_jj_parses() {
        let cli = Cli::try_parse_from(["lumen", "--vcs", "jj", "diff"]).unwrap();
        assert_eq!(cli.vcs, Some(VcsOverride::Jj));
    }

    #[test]
    fn test_vcs_not_specified() {
        let cli = Cli::try_parse_from(["lumen", "diff"]).unwrap();
        assert_eq!(cli.vcs, None);
    }

    #[test]
    fn test_diff_wrap_flag_parses() {
        let cli = Cli::try_parse_from(["lumen", "diff", "--wrap"]).unwrap();
        match cli.command {
            Commands::Diff { wrap, .. } => assert!(wrap),
            _ => panic!("expected diff command"),
        }
    }
}
