use clap::Subcommand;
use console::style;

use crate::config::local::LocalConfig;
use crate::error::{DevflowError, Result};
use crate::git::repo::GitRepo;
use crate::orchestrator::worker as orch_worker;
use crate::tmux::{layout, session, workspace};

#[derive(Subcommand)]
pub enum TmuxCommands {
    /// Attach to the devflow hub tmux session
    Attach,
    /// Attach to a worker's per-task workspace session
    AttachWorker {
        /// Task name of the worker to attach to
        task: String,
    },
    /// Set layout for tmux panes
    Layout {
        /// Layout preset (tiled, even-horizontal, even-vertical, main-horizontal, main-vertical)
        preset: String,
    },
    /// Show tmux session status
    Status,
    /// Generate a default tmux-layout.json template
    InitTemplate,
}

pub async fn run(cmd: TmuxCommands) -> Result<()> {
    match cmd {
        TmuxCommands::Attach => attach().await,
        TmuxCommands::AttachWorker { task } => attach_worker(&task).await,
        TmuxCommands::Layout { preset } => set_layout(&preset).await,
        TmuxCommands::Status => status().await,
        TmuxCommands::InitTemplate => init_template().await,
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

async fn attach_worker(task_name: &str) -> Result<()> {
    if !session::is_available() {
        return Err(DevflowError::TmuxNotAvailable);
    }

    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();
    let local = LocalConfig::load(&devflow_dir.join("local.yml"))?;

    // Check if the worker has a per-task workspace session
    let workers = orch_worker::list_workers(&devflow_dir)?;
    let worker = workers
        .iter()
        .find(|w| w.task_name == task_name)
        .ok_or_else(|| DevflowError::WorkerNotFound(task_name.to_string()))?;

    if let Some(ref ws_name) = worker.tmux_session {
        if session::session_exists(ws_name) {
            session::attach_session(ws_name)?;
            return Ok(());
        }
        println!(
            "Workspace session '{}' not found. Falling back to hub session.",
            ws_name
        );
    }

    // Fall back to hub session
    if session::session_exists(&local.tmux_session_name) {
        session::attach_session(&local.tmux_session_name)?;
    } else {
        println!("No devflow session found. Spawn a worker first.");
    }

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

    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();
    let local = LocalConfig::load(&devflow_dir.join("local.yml"))?;
    let session_name = &local.tmux_session_name;

    if !session::session_exists(session_name) {
        println!("No devflow tmux session active.");
        return Ok(());
    }

    let windows = session::list_windows(session_name)?;
    println!(
        "{} Hub session '{}' with {} window(s):",
        style("✓").green().bold(),
        session_name,
        windows.len()
    );
    for w in &windows {
        println!("  - {w}");
    }

    // Show per-worker sessions
    let workers = orch_worker::list_workers(&devflow_dir)?;
    let workspace_workers: Vec<_> = workers
        .iter()
        .filter(|w| w.tmux_session.is_some())
        .collect();

    if !workspace_workers.is_empty() {
        println!();
        println!("{}", style("Worker workspace sessions:").bold());
        for w in &workspace_workers {
            let ws_name = w.tmux_session.as_ref().unwrap();
            let active = if session::session_exists(ws_name) {
                style("active").green()
            } else {
                style("inactive").red()
            };
            let win_count = if session::session_exists(ws_name) {
                session::list_windows(ws_name).map(|w| w.len()).unwrap_or(0)
            } else {
                0
            };
            println!(
                "  {} {} [{}] ({} windows)",
                style("●").cyan(),
                ws_name,
                active,
                win_count
            );
        }
    }

    Ok(())
}

async fn init_template() -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();

    if !devflow_dir.join("config.yml").exists() {
        return Err(DevflowError::NotInitialized);
    }

    let path = devflow_dir.join("tmux-layout.json");
    if path.exists() {
        println!(
            "{} Template already exists at {}",
            style("!").yellow(),
            path.display()
        );
        println!("  Delete it first if you want to regenerate.");
        return Ok(());
    }

    let template = workspace::default_template();
    let json = serde_json::to_string_pretty(&template)?;
    std::fs::write(&path, json)?;

    println!(
        "{} Created tmux workspace template at {}",
        style("✓").green().bold(),
        path.display()
    );
    println!("  Edit it to customize your per-worker workspace layout.");
    println!(
        "  Workers spawned with a template get a dedicated tmux session with multiple windows/panes."
    );

    Ok(())
}
