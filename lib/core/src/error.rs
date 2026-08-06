use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no jotbay found at {0} (expected a git repository)")]
    NotAJotbay(PathBuf),

    #[error("git {args}: {stderr}")]
    Git { args: String, stderr: String },

    #[error("could not run git, is it installed and on PATH? ({0})")]
    GitMissing(std::io::Error),

    #[error(
        "a previous sync left the repository mid-rebase; \
         run `jotbay resolve --abort` to back out"
    )]
    RebaseInProgress,

    #[error("another sync is already running")]
    Locked,

    #[error("no upstream configured for the current branch")]
    NoUpstream,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
