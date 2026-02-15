use std::path::PathBuf;

use clap::Subcommand;
use console::style;

use crate::config::local::LocalConfig;
use crate::config::project::ProjectConfig;
use crate::error::{TreehouseError, Result};
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
        TreeCommands::Plant { task, task_type, prompt, prompt_file } => {
            plant(&task, &task_type, prompt, prompt_file).await
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

fn ensure_treehouse(git: &GitRepo) -> Result<std::path::PathBuf> {
    let treehouse_dir = git.treehouse_dir();
    if !treehouse_dir.join("config.yml").exists() {
        return Err(TreehouseError::NotInitialized);
    }
    Ok(treehouse_dir)
}

async fn plant(
    task_name: &str,
    task_type: &str,
    prompt: Option<String>,
    prompt_file: Option<PathBuf>,
) -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    let local = LocalConfig::load(&treehouse_dir.join("local.yml"))?;

    // Generate branch name from project config
    let config = ProjectConfig::load(&treehouse_dir.join("config.yml"))?;
    let branch_name = branch::format_branch_name(&config.project_name, task_type, task_name);

    // Resolve prompt text
    let prompt_text = match (prompt, prompt_file) {
        (Some(p), _) => Some(p),
        (_, Some(path)) => {
            let text = std::fs::read_to_string(&path).map_err(|e| {
                TreehouseError::Other(format!("Failed to read prompt file '{}': {e}", path.display()))
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
        &treehouse_dir,
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
    )?;

    println!(
        "{} Tree planted for task '{}'",
        style("✓").green().bold(),
        task_name
    );
    println!("  Branch:   {}", state.branch);
    println!("  Worktree: {}", state.worktree_path.display());

    if let Some(ref ws) = state.tmux_session {
        println!("  Session:  {ws}");
        println!(
            "\nAttach: {}",
            style(format!("th tree attach {task_name}")).cyan()
        );
    }

    Ok(())
}

async fn list() -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    let groves = orch_grove::list_groves(&treehouse_dir)?;
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

        println!(
            "  {} {} [{}] branch:{}{}",
            style("●").cyan(),
            t.task_name,
            status,
            t.branch,
            session_info
        );
    }

    Ok(())
}

async fn status() -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    let groves = orch_grove::list_groves(&treehouse_dir)?;
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
        }
    }

    Ok(())
}

async fn stop(task_name: &str) -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    orch_grove::stop(&treehouse_dir, task_name)?;

    println!(
        "{} Tree '{}' stopped (tmux removed, worktree and branch preserved)",
        style("✓").green().bold(),
        task_name
    );
    println!(
        "  Re-plant with: {}",
        style(format!("th tree plant {task_name}")).cyan()
    );

    Ok(())
}

async fn uproot(task_name: &str, force: bool) -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    orch_grove::uproot(&git, &treehouse_dir, task_name, force)?;

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
            "\n{healthy} healthy, {unhealthy} unhealthy. Run 'th tree prune' to clean up."
        );
    }

    Ok(())
}

async fn attach(task_name: Option<&str>) -> Result<()> {
    if !session::is_available() {
        return Err(TreehouseError::TmuxNotAvailable);
    }

    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;
    let groves = orch_grove::list_groves(&treehouse_dir)?;
    let trees: Vec<_> = groves.iter().filter(|g| g.compose_file.is_none()).collect();

    if trees.is_empty() {
        println!("No active trees. Plant a tree first.");
        return Ok(());
    }

    if let Some(name) = task_name {
        let tree = trees
            .iter()
            .find(|t| t.task_name == name)
            .ok_or_else(|| TreehouseError::GroveNotFound(name.to_string()))?;

        let ws_name = tree.tmux_session.as_ref().ok_or_else(|| {
            TreehouseError::Other(format!("Tree '{name}' has no tmux session"))
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
