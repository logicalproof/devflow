pub mod commit;
pub mod containerize;
pub mod detect;
pub mod grove;
pub mod init;
pub mod task;
pub mod tree;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "th", version, about = "Parallel AI-assisted development orchestrator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new treehouse project
    Init,

    /// Detect project type and frameworks
    Detect,

    /// Manage tasks
    #[command(subcommand)]
    Task(task::TaskCommands),

    /// Containerized development environments
    #[command(subcommand)]
    Grove(grove::GroveCommands),

    /// Lightweight worktrees (no containers)
    #[command(subcommand)]
    Tree(tree::TreeCommands),

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
        Commands::Grove(cmd) => grove::run(cmd).await,
        Commands::Tree(cmd) => tree::run(cmd).await,
        Commands::Containerize => containerize::run().await,
        Commands::Commit => commit::run().await,
    }
}
