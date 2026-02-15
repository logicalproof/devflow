use clap::Subcommand;
use console::style;

use crate::claude_md;
use crate::config::local::LocalConfig;
use crate::error::{DevflowError, Result};
use crate::git::repo::GitRepo;
use crate::orchestrator::worker as orch_worker;
use crate::tmux::{layout, session, workspace};

#[derive(Subcommand)]
pub enum TmuxCommands {
    /// Attach to a worker's tmux session (picks first if no task specified)
    Attach {
        /// Task name of the worker to attach to (optional — attaches to first worker if omitted)
        task: Option<String>,
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
    /// Generate a default claude-md.template for customization
    InitClaudeTemplate,
}

pub async fn run(cmd: TmuxCommands) -> Result<()> {
    match cmd {
        TmuxCommands::Attach { task } => attach(task.as_deref()).await,
        TmuxCommands::Layout { preset } => set_layout(&preset).await,
        TmuxCommands::Status => status().await,
        TmuxCommands::InitTemplate => init_template().await,
        TmuxCommands::InitClaudeTemplate => init_claude_template().await,
    }
}

async fn attach(task_name: Option<&str>) -> Result<()> {
    if !session::is_available() {
        return Err(DevflowError::TmuxNotAvailable);
    }

    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();
    let workers = orch_worker::list_workers(&devflow_dir)?;

    if workers.is_empty() {
        println!("No active workers. Spawn a worker first.");
        return Ok(());
    }

    // If a task name is given, attach to that worker's session
    if let Some(name) = task_name {
        let worker = workers
            .iter()
            .find(|w| w.task_name == name)
            .ok_or_else(|| DevflowError::WorkerNotFound(name.to_string()))?;

        let ws_name = worker.tmux_session.as_ref().ok_or_else(|| {
            DevflowError::Other(format!("Worker '{name}' has no tmux session"))
        })?;

        if session::session_exists(ws_name) {
            session::attach_session(ws_name)?;
        } else {
            println!("Session '{ws_name}' no longer exists.");
        }
        return Ok(());
    }

    // No task specified — attach to first worker's session
    for w in &workers {
        if let Some(ref ws_name) = w.tmux_session {
            if session::session_exists(ws_name) {
                session::attach_session(ws_name)?;
                return Ok(());
            }
        }
    }

    println!("No active worker sessions found.");
    Ok(())
}

async fn set_layout(preset: &str) -> Result<()> {
    if !session::is_available() {
        return Err(DevflowError::TmuxNotAvailable);
    }

    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();
    let local = LocalConfig::load(&devflow_dir.join("local.yml"))?;
    layout::apply_layout(&local.tmux_session_name, preset)?;

    println!(
        "{} Applied layout '{}'",
        style("✓").green().bold(),
        preset,
    );
    Ok(())
}

async fn status() -> Result<()> {
    if !session::is_available() {
        return Err(DevflowError::TmuxNotAvailable);
    }

    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();
    let workers = orch_worker::list_workers(&devflow_dir)?;

    if workers.is_empty() {
        println!("No active workers.");
        return Ok(());
    }

    println!("{}", style("Worker sessions:").bold());
    for w in &workers {
        if let Some(ref ws_name) = w.tmux_session {
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
        } else {
            println!(
                "  {} {} [{}]",
                style("●").red(),
                w.task_name,
                style("no session").red()
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

    Ok(())
}

async fn init_claude_template() -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();

    if !devflow_dir.join("config.yml").exists() {
        return Err(DevflowError::NotInitialized);
    }

    let path = devflow_dir.join("claude-md.template");
    if path.exists() {
        println!(
            "{} Template already exists at {}",
            style("!").yellow(),
            path.display()
        );
        println!("  Delete it first if you want to regenerate.");
        return Ok(());
    }

    std::fs::write(&path, claude_md::default_template())?;

    println!(
        "{} Created CLAUDE.local.md template at {}",
        style("✓").green().bold(),
        path.display()
    );
    println!("  Edit it to customize the CLAUDE.local.md generated in each worker's worktree.");
    println!("  Available variables: {{{{WORKTREE_PATH}}}}, {{{{WORKER_NAME}}}}, {{{{BRANCH_NAME}}}},");
    println!("    {{{{PROJECT_NAME}}}}, {{{{TASK_TYPE}}}}, {{{{DETECTED_TYPES}}}}, {{{{COMPOSE_FILE}}}},");
    println!("    {{{{COMPOSE_PROJECT}}}}, {{{{APP_PORT}}}}, {{{{DB_PORT}}}}, {{{{REDIS_PORT}}}}");
    println!("  Conditionals: {{{{#if COMPOSE_ENABLED}}}}...{{{{/if}}}}, {{{{#if !COMPOSE_ENABLED}}}}...{{{{/if}}}}");

    Ok(())
}
