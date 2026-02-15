use std::path::PathBuf;

use clap::Subcommand;
use console::style;

use crate::compose::db as compose_db;
use crate::config::local::LocalConfig;
use crate::error::{DevflowError, Result};
use crate::git::repo::GitRepo;
use crate::orchestrator::{cleanup, state::WorkerState, worker as orch_worker};

use super::task::{self, TaskState};

#[derive(Subcommand)]
pub enum WorkerCommands {
    /// Spawn a new worker for a task
    Spawn {
        /// Task name to spawn worker for
        task: String,
        /// Launch claude with this prompt in the worker's tmux window
        #[arg(long)]
        prompt: Option<String>,
        /// Launch claude with the prompt read from this file
        #[arg(long, conflicts_with = "prompt")]
        prompt_file: Option<PathBuf>,
        /// Start a Docker Compose stack for this worker (isolated app/db/redis)
        #[arg(long)]
        compose: bool,
        /// Clone the host's development database into the worker instead of running db:prepare
        #[arg(long, requires = "compose")]
        db_clone: bool,
        /// Source database URL for --db-clone (default: auto-detect from config/database.yml)
        #[arg(long, requires = "db_clone")]
        db_source: Option<String>,
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
    /// Clean up orphaned workers (containers, worktrees, state)
    Cleanup,
    /// Clone the host database into a running worker's compose stack
    DbClone {
        /// Task name of the worker
        task: String,
        /// Source database URL (default: auto-detect from config/database.yml)
        #[arg(long)]
        source: Option<String>,
    },
}

pub async fn run(cmd: WorkerCommands) -> Result<()> {
    match cmd {
        WorkerCommands::Spawn {
            task,
            prompt,
            prompt_file,
            compose,
            db_clone,
            db_source,
        } => spawn(&task, prompt, prompt_file, compose, db_clone, db_source).await,
        WorkerCommands::List => list().await,
        WorkerCommands::Kill { task } => kill(&task).await,
        WorkerCommands::Monitor => monitor().await,
        WorkerCommands::Cleanup => cleanup_cmd().await,
        WorkerCommands::DbClone { task, source } => db_clone_cmd(&task, source).await,
    }
}

fn ensure_devflow(git: &GitRepo) -> Result<std::path::PathBuf> {
    let devflow_dir = git.devflow_dir();
    if !devflow_dir.join("config.yml").exists() {
        return Err(DevflowError::NotInitialized);
    }
    Ok(devflow_dir)
}

async fn spawn(
    task_name: &str,
    prompt: Option<String>,
    prompt_file: Option<PathBuf>,
    enable_compose: bool,
    db_clone: bool,
    db_source: Option<String>,
) -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;

    // Clean up orphans on spawn
    let local = LocalConfig::load(&devflow_dir.join("local.yml"))?;
    let _ = cleanup_orphans(&devflow_dir, &git);

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

    // Resolve prompt text
    let prompt_text = match (prompt, prompt_file) {
        (Some(p), _) => Some(p),
        (_, Some(path)) => {
            let text = std::fs::read_to_string(&path).map_err(|e| {
                DevflowError::Other(format!("Failed to read prompt file '{}': {e}", path.display()))
            })?;
            Some(text)
        }
        _ => None,
    };

    // Build claude command if prompt provided
    let initial_command = prompt_text.as_ref().map(|text: &String| {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        format!("claude --prompt \"{escaped}\"")
    });

    // Resolve db_source: CLI flag > local.yml config > auto-detect (handled in orchestrator)
    let resolved_db_source = db_source.or(local.compose_db_source);

    // Spawn the worker
    let state = orch_worker::spawn(
        &git,
        &devflow_dir,
        task_name,
        &branch_name,
        &local.tmux_session_name,
        local.min_disk_space_mb,
        initial_command.as_deref(),
        enable_compose,
        local.compose_health_timeout_secs,
        &local.compose_post_start,
        db_clone,
        resolved_db_source.as_deref(),
    )?;

    println!(
        "{} Worker spawned for task '{}'",
        style("✓").green().bold(),
        task_name
    );
    println!("  Branch:   {}", state.branch);
    println!("  Worktree: {}", state.worktree_path.display());

    if let Some(ref ws) = state.tmux_session {
        println!("  Session:  {ws}");
    }

    if let Some(ref ports) = state.compose_ports {
        println!("  Compose:");
        println!("    App:   http://localhost:{}", ports.app);
        println!("    DB:    localhost:{}", ports.db);
        println!("    Redis: localhost:{}", ports.redis);
    }

    if let Some(ref ws) = state.tmux_session {
        println!(
            "\nAttach: {}",
            style(format!("tmux attach -t {ws}")).cyan()
        );
    }

    Ok(())
}

async fn list() -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;
    let _ = cleanup_orphans(&devflow_dir, &git);

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

        let compose_info = if let Some(ref ports) = w.compose_ports {
            format!(" [compose: {}:{}:{}]", ports.app, ports.db, ports.redis)
        } else {
            String::new()
        };

        let session_info = w
            .tmux_session
            .as_ref()
            .map(|s| format!(" session:{s}"))
            .unwrap_or_default();

        println!(
            "  {} {} [{}] branch:{} worktree:{}{}{}",
            style("●").cyan(),
            w.task_name,
            status,
            w.branch,
            w.worktree_path.display(),
            session_info,
            compose_info
        );
    }

    Ok(())
}

async fn kill(task_name: &str) -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;

    orch_worker::kill(&git, &devflow_dir, task_name)?;

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
    let _ = cleanup_orphans(&devflow_dir, &git);

    let workers = orch_worker::list_workers(&devflow_dir)?;

    println!("{}", style("Worker Monitor").bold());
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
            if let Some(ref ws) = w.tmux_session {
                println!("    Session:  {ws}");
            }
            if let Some(ref ports) = w.compose_ports {
                println!(
                    "    Compose:  app:{} db:{} redis:{}",
                    ports.app, ports.db, ports.redis
                );
            }
        }
    }

    Ok(())
}

async fn cleanup_cmd() -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;

    let orphans = cleanup::find_orphans(&devflow_dir)?;

    if orphans.is_empty() {
        println!("No orphaned workers found.");
        return Ok(());
    }

    println!(
        "{} Found {} orphaned worker(s):",
        style("!").yellow(),
        orphans.len()
    );
    for o in &orphans {
        let compose_info = if o.compose_file.is_some() {
            " [compose stack running]"
        } else {
            ""
        };
        println!(
            "  {} {} branch:{} worktree:{}{}",
            style("●").red(),
            o.task_name,
            o.branch,
            o.worktree_path.display(),
            compose_info
        );
    }

    for o in &orphans {
        cleanup::cleanup_orphan(&devflow_dir, &git.root, o)?;
    }

    println!(
        "{} Cleaned up {} orphaned worker(s)",
        style("✓").green().bold(),
        orphans.len()
    );

    Ok(())
}

async fn db_clone_cmd(task_name: &str, source: Option<String>) -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;
    let local = LocalConfig::load(&devflow_dir.join("local.yml"))?;

    // Load worker state
    let state_path = WorkerState::state_path(&devflow_dir, task_name);
    if !state_path.exists() {
        return Err(DevflowError::WorkerNotFound(task_name.to_string()));
    }
    let state = WorkerState::load(&state_path)?;

    // Verify worker has a compose stack
    let compose_file = state.compose_file.as_ref().ok_or_else(|| {
        DevflowError::Other(format!(
            "Worker '{task_name}' was not started with --compose. \
             Database cloning requires a compose stack."
        ))
    })?;

    // Resolve source: CLI flag > config > auto-detect from worktree
    let source_url = if let Some(src) = source {
        src
    } else if let Some(ref src) = local.compose_db_source {
        println!("Using configured source: {src}");
        src.clone()
    } else {
        let url = compose_db::detect_source_db(&state.worktree_path)?;
        println!("Auto-detected source database: {url}");
        url
    };

    compose_db::clone_database(compose_file, &source_url, task_name)?;

    println!(
        "{} Database cloned into worker '{task_name}'",
        style("✓").green().bold(),
    );

    Ok(())
}

fn cleanup_orphans(
    devflow_dir: &std::path::Path,
    git: &GitRepo,
) -> Result<()> {
    let orphans = cleanup::find_orphans(devflow_dir)?;
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
