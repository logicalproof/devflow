use thiserror::Error;

#[derive(Error, Debug)]
pub enum DevflowError {
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

    #[error("Not a devflow project. Run 'devflow init' first.")]
    NotInitialized,

    #[error("Not a git repository")]
    NotGitRepo,

    #[error("Git command failed: {0}")]
    GitCommand(String),

    #[error("Tmux command failed: {0}")]
    TmuxCommand(String),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Task already exists: {0}")]
    TaskAlreadyExists(String),

    #[error("Invalid task state transition: {current} -> {target}")]
    InvalidTaskState { current: String, target: String },

    #[error("Worker already exists for task: {0}")]
    WorkerAlreadyExists(String),

    #[error("Worker not found: {0}")]
    WorkerNotFound(String),

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

    #[error("Tmux is not available")]
    TmuxNotAvailable,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DevflowError>;
