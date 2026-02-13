use clap::Subcommand;
use console::style;

use crate::config::local::LocalConfig;
use crate::error::{DevflowError, Result};
use crate::git::repo::GitRepo;
use crate::tmux::{layout, session};

#[derive(Subcommand)]
pub enum TmuxCommands {
    /// Attach to the devflow tmux session
    Attach,
    /// Set layout for tmux panes
    Layout {
        /// Layout preset (tiled, even-horizontal, even-vertical, main-horizontal, main-vertical)
        preset: String,
    },
    /// Show tmux session status
    Status,
}

pub async fn run(cmd: TmuxCommands) -> Result<()> {
    match cmd {
        TmuxCommands::Attach => attach().await,
        TmuxCommands::Layout { preset } => set_layout(&preset).await,
        TmuxCommands::Status => status().await,
    }
}

fn load_session_name() -> Result<String> {
    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();
    let local = LocalConfig::load(&devflow_dir.join("local.yml"))?;
    Ok(local.tmux_session_name)
}

async fn attach() -> Result<()> {
    if !session::is_available() {
        return Err(DevflowError::TmuxNotAvailable);
    }

    let session_name = load_session_name()?;

    if !session::session_exists(&session_name) {
        println!("No devflow session found. Spawn a worker first.");
        return Ok(());
    }

    session::attach_session(&session_name)?;
    Ok(())
}

async fn set_layout(preset: &str) -> Result<()> {
    if !session::is_available() {
        return Err(DevflowError::TmuxNotAvailable);
    }

    let session_name = load_session_name()?;
    layout::apply_layout(&session_name, preset)?;

    println!(
        "{} Applied layout '{}' to session '{}'",
        style("✓").green().bold(),
        preset,
        session_name
    );
    Ok(())
}

async fn status() -> Result<()> {
    if !session::is_available() {
        return Err(DevflowError::TmuxNotAvailable);
    }

    let session_name = load_session_name()?;

    if !session::session_exists(&session_name) {
        println!("No devflow tmux session active.");
        return Ok(());
    }

    let windows = session::list_windows(&session_name)?;
    println!(
        "{} Session '{}' with {} window(s):",
        style("✓").green().bold(),
        session_name,
        windows.len()
    );
    for w in &windows {
        println!("  - {w}");
    }
    Ok(())
}
