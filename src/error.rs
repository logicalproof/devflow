use thiserror::Error;

#[derive(Error, Debug)]
pub enum GrootError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Git error: {0}")]
    Git(#[from] git2::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yml::Error),

    #[error("Docker error: {0}")]
    Docker(#[from] bollard::errors::Error),

    #[error("Not a groot project. Run 'groot init' first.")]
    NotInitialized,

    #[error("Not a git repository")]
    NotGitRepo,

    #[error("Git command failed: {0}")]
    GitCommand(String),

    #[error("Tmux command failed: {0}")]
    TmuxCommand(String),

    #[error("Grove already exists for task: {0}")]
    GroveAlreadyExists(String),

    #[error("Grove not found: {0}")]
    GroveNotFound(String),

    #[error("Worktree already exists: {0}")]
    WorktreeAlreadyExists(String),

    #[error("Branch already exists: {0}")]
    BranchAlreadyExists(String),

    #[error("Insufficient disk space: {available_mb}MB available, {required_mb}MB required")]
    InsufficientDiskSpace { available_mb: u64, required_mb: u64 },

    #[error("Lock acquisition failed: {0}")]
    LockFailed(String),

    #[error("Container not found: {0}")]
    ContainerNotFound(String),

    #[error("Template not found: {0}")]
    TemplateNotFound(String),

    #[error("Docker is not available")]
    DockerNotAvailable,

    #[error("Docker Compose is not available. Install: https://docs.docker.com/compose/install/")]
    ComposeNotAvailable,

    #[error("Port {port} is already in use ({service}). Free it or adjust the compose template.")]
    PortInUse { port: u16, service: String },

    #[error("Compose operation failed: {0}")]
    ComposeOperationFailed(String),

    #[error("Tmux is not available")]
    TmuxNotAvailable,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, GrootError>;
