pub mod commit;
pub mod container;
pub mod containerize;
pub mod detect;
pub mod init;
pub mod task;
pub mod tmux;
pub mod worker;
pub mod worktree;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "devflow", version, about = "Parallel AI-assisted development orchestrator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new devflow project
    Init,

    /// Detect project type and frameworks
    Detect,

    /// Manage tasks
    #[command(subcommand)]
    Task(task::TaskCommands),

    /// Manage workers (parallel dev environments)
    #[command(subcommand)]
    Worker(worker::WorkerCommands),

    /// Manage git worktrees
    #[command(subcommand)]
    Worktree(worktree::WorktreeCommands),

    /// Manage tmux sessions
    #[command(subcommand)]
    Tmux(tmux::TmuxCommands),

    /// Manage Docker containers
    #[command(subcommand)]
    Container(container::ContainerCommands),

    /// Interactive container setup wizard
    Containerize,

    /// Interactive conventional commit helper
    Commit,
}

pub async fn dispatch(cmd: Commands) -> crate::error::Result<()> {
    match cmd {
        Commands::Init => init::run().await,
        Commands::Detect => detect::run().await,
        Commands::Task(cmd) => task::run(cmd).await,
        Commands::Worker(cmd) => worker::run(cmd).await,
        Commands::Worktree(cmd) => worktree::run(cmd).await,
        Commands::Tmux(cmd) => tmux::run(cmd).await,
        Commands::Container(cmd) => container::run(cmd).await,
        Commands::Containerize => containerize::run().await,
        Commands::Commit => commit::run().await,
    }
}
