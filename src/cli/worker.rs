use clap::Subcommand;
use console::style;

use crate::config::local::LocalConfig;
use crate::error::{DevflowError, Result};
use crate::git::repo::GitRepo;
use crate::orchestrator::{cleanup, worker as orch_worker};

use super::task::{self, TaskState};

#[derive(Subcommand)]
pub enum WorkerCommands {
    /// Spawn a new worker for a task
    Spawn {
        /// Task name to spawn worker for
        task: String,
    },
    /// List all active workers
    List,
    /// Kill a worker and clean up resources
    Kill {
        /// Task name of the worker to kill
        task: String,
    },
    /// Show worker status and resource usage
    Monitor,
}

pub async fn run(cmd: WorkerCommands) -> Result<()> {
    match cmd {
        WorkerCommands::Spawn { task } => spawn(&task).await,
        WorkerCommands::List => list().await,
        WorkerCommands::Kill { task } => kill(&task).await,
        WorkerCommands::Monitor => monitor().await,
    }
}

fn ensure_devflow(git: &GitRepo) -> Result<std::path::PathBuf> {
    let devflow_dir = git.devflow_dir();
    if !devflow_dir.join("config.yml").exists() {
        return Err(DevflowError::NotInitialized);
    }
    Ok(devflow_dir)
}

async fn spawn(task_name: &str) -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;

    // Clean up orphans on spawn
    let local = LocalConfig::load(&devflow_dir.join("local.yml"))?;
    let _ = cleanup_orphans(&devflow_dir, &git, &local.tmux_session_name);

    // Load task to get branch name
    let tasks_path = devflow_dir.join("tasks.json");
    let contents = std::fs::read_to_string(&tasks_path)?;
    let mut tasks: Vec<task::Task> = serde_json::from_str(&contents)?;

    let task = tasks
        .iter_mut()
        .find(|t| t.name == task_name)
        .ok_or_else(|| DevflowError::TaskNotFound(task_name.to_string()))?;

    if task.state == TaskState::Closed {
        return Err(DevflowError::InvalidTaskState {
            current: task.state.to_string(),
            target: "spawn worker".to_string(),
        });
    }

    let branch_name = task.branch.clone();

    // Set task to active
    task.state = TaskState::Active;
    task.updated_at = chrono::Utc::now();
    let tasks_json = serde_json::to_string_pretty(&tasks)?;
    std::fs::write(&tasks_path, tasks_json)?;

    // Spawn the worker
    let state = orch_worker::spawn(
        &git,
        &devflow_dir,
        task_name,
        &branch_name,
        &local.tmux_session_name,
        local.min_disk_space_mb,
    )?;

    println!(
        "{} Worker spawned for task '{}'",
        style("✓").green().bold(),
        task_name
    );
    println!("  Branch:   {}", state.branch);
    println!("  Worktree: {}", state.worktree_path.display());
    println!("  Tmux:     {}:{}", local.tmux_session_name, state.tmux_window);
    println!(
        "\nAttach with: {}",
        style(format!("devflow tmux attach")).cyan()
    );

    Ok(())
}

async fn list() -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;

    let workers = orch_worker::list_workers(&devflow_dir)?;

    if workers.is_empty() {
        println!("No active workers.");
        return Ok(());
    }

    println!("{}", style("Active workers:").bold());
    for w in &workers {
        let worktree_ok = w.worktree_path.exists();
        let status = if worktree_ok {
            style("running").green()
        } else {
            style("orphaned").red()
        };
        println!(
            "  {} {} [{}] branch:{} worktree:{}",
            style("●").cyan(),
            w.task_name,
            status,
            w.branch,
            w.worktree_path.display()
        );
    }

    Ok(())
}

async fn kill(task_name: &str) -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;
    let local = LocalConfig::load(&devflow_dir.join("local.yml"))?;

    orch_worker::kill(&git, &devflow_dir, task_name, &local.tmux_session_name)?;

    println!(
        "{} Worker '{}' killed and resources cleaned up",
        style("✓").green().bold(),
        task_name
    );

    Ok(())
}

async fn monitor() -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;
    let local = LocalConfig::load(&devflow_dir.join("local.yml"))?;

    let workers = orch_worker::list_workers(&devflow_dir)?;

    println!("{}", style("Worker Monitor").bold());
    println!("Session: {}", local.tmux_session_name);
    println!("Max workers: {}", local.max_workers);
    println!("Active: {}/{}", workers.len(), local.max_workers);
    println!();

    if workers.is_empty() {
        println!("No active workers.");
    } else {
        for w in &workers {
            let age = chrono::Utc::now() - w.created_at;
            let hours = age.num_hours();
            let mins = age.num_minutes() % 60;
            println!(
                "  {} {} (uptime: {}h {}m)",
                style("●").green(),
                w.task_name,
                hours,
                mins
            );
            println!("    Branch:   {}", w.branch);
            println!("    Worktree: {}", w.worktree_path.display());
        }
    }

    Ok(())
}

fn cleanup_orphans(
    devflow_dir: &std::path::Path,
    git: &GitRepo,
    tmux_session: &str,
) -> Result<()> {
    let orphans = cleanup::find_orphans(devflow_dir, tmux_session)?;
    for orphan in &orphans {
        cleanup::cleanup_orphan(devflow_dir, &git.root, orphan)?;
    }
    if !orphans.is_empty() {
        println!(
            "{} Cleaned up {} orphaned worker(s)",
            style("!").yellow(),
            orphans.len()
        );
    }
    Ok(())
}
