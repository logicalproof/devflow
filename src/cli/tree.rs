use std::path::PathBuf;

use clap::Subcommand;
use console::style;

use crate::config::local::LocalConfig;
use crate::config::project::ProjectConfig;
use crate::error::{GrootError, Result};
use crate::git::{branch, repo::GitRepo, worktree as wt};
use crate::orchestrator::grove as orch_grove;
use crate::tmux::session;

#[derive(Subcommand)]
pub enum TreeCommands {
    /// Plant a new lightweight worktree for a task (no containers)
    Plant {
        /// Task name
        task: String,
        /// Task type (feature, bugfix, refactor, chore)
        #[arg(short = 't', long = "type", default_value = "feature")]
        task_type: String,
        /// Launch claude with this prompt in the tree's tmux window
        #[arg(long)]
        prompt: Option<String>,
        /// Launch claude with the prompt read from this file
        #[arg(long, conflicts_with = "prompt")]
        prompt_file: Option<PathBuf>,
        /// Share a running grove's compose stack (db, redis) instead of running bare
        #[arg(short = 'g', long)]
        grove: Option<String>,
    },
    /// List all trees
    List,
    /// Show tree status and health
    Status,
    /// Stop a tree (tear down tmux but keep worktree and branch)
    Stop {
        /// Task name of the tree to stop
        task: String,
    },
    /// Uproot a tree and clean up all resources (worktree, branch, tmux)
    Uproot {
        /// Task name of the tree to uproot
        task: String,
        /// Force uproot even if the worktree has uncommitted changes or unpushed commits
        #[arg(long)]
        force: bool,
    },
    /// Clean up stale worktrees
    Prune,
    /// Check worktree health
    Health,
    /// Attach to a tree's tmux session
    Attach {
        /// Task name of the tree to attach to (optional — attaches to first tree if omitted)
        task: Option<String>,
    },
}

pub async fn run(cmd: TreeCommands) -> Result<()> {
    match cmd {
        TreeCommands::Plant { task, task_type, prompt, prompt_file, grove } => {
            plant(&task, &task_type, prompt, prompt_file, grove).await
        }
        TreeCommands::List => list().await,
        TreeCommands::Status => status().await,
        TreeCommands::Stop { task } => stop(&task).await,
        TreeCommands::Uproot { task, force } => uproot(&task, force).await,
        TreeCommands::Prune => prune().await,
        TreeCommands::Health => health().await,
        TreeCommands::Attach { task } => attach(task.as_deref()).await,
    }
}

fn ensure_groot(git: &GitRepo) -> Result<std::path::PathBuf> {
    let groot_dir = git.groot_dir();
    if !groot_dir.join("config.yml").exists() {
        return Err(GrootError::NotInitialized);
    }
    Ok(groot_dir)
}

/// If the cwd is inside a grove's worktree, return that grove's task name.
fn detect_grove_from_cwd(groot_dir: &std::path::Path) -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let worktrees_dir = groot_dir.join("worktrees");
    let relative = cwd.strip_prefix(&worktrees_dir).ok()?;
    // First component of the relative path is the task name
    let task_name = relative.components().next()?.as_os_str().to_str()?;
    // Check if it's actually a grove (has compose_file)
    let state = orch_grove::get_grove_by_name(groot_dir, task_name).ok()?;
    if state.compose_file.is_some() {
        Some(task_name.to_string())
    } else {
        None
    }
}

async fn plant(
    task_name: &str,
    task_type: &str,
    prompt: Option<String>,
    prompt_file: Option<PathBuf>,
    grove: Option<String>,
) -> Result<()> {
    let git = GitRepo::discover()?;
    let groot_dir = ensure_groot(&git)?;

    let local = LocalConfig::load(&groot_dir.join("local.yml"))?;

    // Generate branch name from project config
    let config = ProjectConfig::load(&groot_dir.join("config.yml"))?;
    let branch_name = branch::format_branch_name(&config.project_name, task_type, task_name);

    // Resolve grove: explicit --grove flag, or auto-detect from cwd inside a grove worktree
    let auto_detected = grove.is_none();
    let grove = grove.or_else(|| detect_grove_from_cwd(&groot_dir));

    // If grove is set, validate it exists and has a running compose stack
    let (shared_grove_name, shared_ports) = if let Some(ref grove_name) = grove {
        let grove_state = orch_grove::get_grove_by_name(&groot_dir, grove_name)?;

        if grove_state.compose_file.is_none() {
            return Err(GrootError::Other(format!(
                "'{grove_name}' is not a grove (no compose stack). \
                 Use --grove with a grove that has a running compose stack."
            )));
        }

        // Verify compose stack is running (tmux session exists as proxy)
        if let Some(ref ws) = grove_state.tmux_session {
            if !session::session_exists(ws) {
                return Err(GrootError::Other(format!(
                    "Grove '{grove_name}' tmux session is not running. \
                     Start it first with: groot grove plant {grove_name}"
                )));
            }
        }

        let ports = grove_state.compose_ports.ok_or_else(|| {
            GrootError::Other(format!(
                "Grove '{grove_name}' has no allocated ports."
            ))
        })?;

        if auto_detected {
            println!(
                "Auto-detected grove '{}' from current worktree",
                grove_name
            );
        }

        (Some(grove_name.as_str()), Some(ports))
    } else {
        (None, None)
    };

    // Resolve prompt text
    let prompt_text = match (prompt, prompt_file) {
        (Some(p), _) => Some(p),
        (_, Some(path)) => {
            let text = std::fs::read_to_string(&path).map_err(|e| {
                GrootError::Other(format!("Failed to read prompt file '{}': {e}", path.display()))
            })?;
            Some(text)
        }
        _ => None,
    };

    let initial_command = prompt_text.as_ref().map(|text: &String| {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        format!("claude --prompt \"{escaped}\"")
    });

    // Plant the tree (no compose)
    let state = orch_grove::plant(
        &git,
        &groot_dir,
        task_name,
        &branch_name,
        task_type,
        &local.tmux_session_name,
        local.min_disk_space_mb,
        initial_command.as_deref(),
        false, // never compose for tree
        0,
        &[],
        false,
        None,
        shared_grove_name,
        shared_ports.as_ref(),
    )?;

    println!(
        "{} Tree planted for task '{}'",
        style("✓").green().bold(),
        task_name
    );
    println!("  Branch:   {}", state.branch);
    println!("  Worktree: {}", state.worktree_path.display());

    if let Some(ref grove_name) = grove {
        println!("  Shared:   grove '{grove_name}'");
        if let Some(ref ports) = state.shared_compose_ports {
            println!("  Ports:    app:{} db:{} redis:{}", ports.app, ports.db, ports.redis);
        }
    }

    if let Some(ref ws) = state.tmux_session {
        println!("  Session:  {ws}");
        println!(
            "\nAttach: {}",
            style(format!("groot tree attach {task_name}")).cyan()
        );
    }

    Ok(())
}

async fn list() -> Result<()> {
    let git = GitRepo::discover()?;
    let groot_dir = ensure_groot(&git)?;

    let groves = orch_grove::list_groves(&groot_dir)?;
    let trees: Vec<_> = groves.iter().filter(|g| g.compose_file.is_none()).collect();

    if trees.is_empty() {
        println!("No active trees.");
        return Ok(());
    }

    println!("{}", style("Active trees:").bold());
    for t in &trees {
        let worktree_ok = t.worktree_path.exists();
        let status = if worktree_ok {
            style("ok").green()
        } else {
            style("missing").red()
        };

        let session_info = t
            .tmux_session
            .as_ref()
            .map(|s| format!(" session:{s}"))
            .unwrap_or_default();

        let shared_info = t
            .shared_grove
            .as_ref()
            .map(|g| format!(" [shared: {g}]"))
            .unwrap_or_default();

        println!(
            "  {} {} [{}] branch:{}{}{}",
            style("●").cyan(),
            t.task_name,
            status,
            t.branch,
            session_info,
            shared_info
        );
    }

    Ok(())
}

async fn status() -> Result<()> {
    let git = GitRepo::discover()?;
    let groot_dir = ensure_groot(&git)?;

    let groves = orch_grove::list_groves(&groot_dir)?;
    let trees: Vec<_> = groves.iter().filter(|g| g.compose_file.is_none()).collect();

    println!("{}", style("Tree Status").bold());
    println!("Active trees: {}", trees.len());
    println!();

    if trees.is_empty() {
        println!("No active trees.");
    } else {
        for t in &trees {
            let age = chrono::Utc::now() - t.created_at;
            let hours = age.num_hours();
            let mins = age.num_minutes() % 60;
            println!(
                "  {} {} (uptime: {}h {}m)",
                style("●").green(),
                t.task_name,
                hours,
                mins
            );
            println!("    Branch:   {}", t.branch);
            println!("    Worktree: {}", t.worktree_path.display());
            if let Some(ref ws) = t.tmux_session {
                let active = if session::session_exists(ws) {
                    style("active").green()
                } else {
                    style("inactive").red()
                };
                println!("    Session:  {ws} [{active}]");
            }
            if let Some(ref grove_name) = t.shared_grove {
                println!("    Shared grove: {grove_name}");
                if let Some(ref ports) = t.shared_compose_ports {
                    println!(
                        "    Shared ports: app:{} db:{} redis:{}",
                        ports.app, ports.db, ports.redis
                    );
                }
            }
        }
    }

    Ok(())
}

async fn stop(task_name: &str) -> Result<()> {
    let git = GitRepo::discover()?;
    let groot_dir = ensure_groot(&git)?;

    orch_grove::stop(&groot_dir, task_name, false)?;

    println!(
        "{} Tree '{}' stopped (tmux removed, worktree and branch preserved)",
        style("✓").green().bold(),
        task_name
    );
    println!(
        "  Re-plant with: {}",
        style(format!("groot tree plant {task_name}")).cyan()
    );

    Ok(())
}

async fn uproot(task_name: &str, force: bool) -> Result<()> {
    let git = GitRepo::discover()?;
    let groot_dir = ensure_groot(&git)?;

    orch_grove::uproot(&git, &groot_dir, task_name, force)?;

    println!(
        "{} Tree '{}' uprooted and resources cleaned up",
        style("✓").green().bold(),
        task_name
    );

    Ok(())
}

async fn prune() -> Result<()> {
    let git = GitRepo::discover()?;
    wt::prune_worktrees(&git.root)?;
    println!("{} Pruned stale worktree entries", style("✓").green().bold());
    Ok(())
}

async fn health() -> Result<()> {
    let git = GitRepo::discover()?;
    let worktrees = wt::list_worktrees(&git.root)?;

    let mut healthy = 0;
    let mut unhealthy = 0;

    for w in &worktrees {
        if w.path.exists() && wt::worktree_exists(&w.path) {
            healthy += 1;
        } else if !w.bare {
            unhealthy += 1;
            println!(
                "  {} {} - path missing or invalid",
                style("✗").red().bold(),
                w.path.display()
            );
        }
    }

    if unhealthy == 0 {
        println!(
            "{} All {healthy} worktrees healthy",
            style("✓").green().bold()
        );
    } else {
        println!(
            "\n{healthy} healthy, {unhealthy} unhealthy. Run 'groot tree prune' to clean up."
        );
    }

    Ok(())
}

async fn attach(task_name: Option<&str>) -> Result<()> {
    if !session::is_available() {
        return Err(GrootError::TmuxNotAvailable);
    }

    let git = GitRepo::discover()?;
    let groot_dir = ensure_groot(&git)?;
    let groves = orch_grove::list_groves(&groot_dir)?;
    let trees: Vec<_> = groves.iter().filter(|g| g.compose_file.is_none()).collect();

    if trees.is_empty() {
        println!("No active trees. Plant a tree first.");
        return Ok(());
    }

    if let Some(name) = task_name {
        let tree = trees
            .iter()
            .find(|t| t.task_name == name)
            .ok_or_else(|| GrootError::GroveNotFound(name.to_string()))?;

        let ws_name = tree.tmux_session.as_ref().ok_or_else(|| {
            GrootError::Other(format!("Tree '{name}' has no tmux session"))
        })?;

        if session::session_exists(ws_name) {
            session::attach_session(ws_name)?;
        } else {
            println!("Session '{ws_name}' no longer exists.");
        }
        return Ok(());
    }

    // No task specified — attach to first tree's session
    for t in &trees {
        if let Some(ref ws_name) = t.tmux_session {
            if session::session_exists(ws_name) {
                session::attach_session(ws_name)?;
                return Ok(());
            }
        }
    }

    println!("No active tree sessions found.");
    Ok(())
}
