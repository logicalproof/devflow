use std::path::PathBuf;

use clap::Subcommand;
use console::style;

use crate::claude_md;
use crate::compose::db as compose_db;
use crate::config::local::LocalConfig;
use crate::container::docker::DockerClient;
use crate::error::{TreehouseError, Result};
use crate::git::repo::GitRepo;
use crate::orchestrator::{cleanup, state::GroveState, grove as orch_grove};
use crate::tmux::{layout, session, workspace};

use super::task::{self, TaskState};

#[derive(Subcommand)]
pub enum GroveCommands {
    /// Plant a new containerized environment for a task
    Plant {
        /// Task name
        task: String,
        /// Launch claude with this prompt in the grove's tmux window
        #[arg(long)]
        prompt: Option<String>,
        /// Launch claude with the prompt read from this file
        #[arg(long, conflicts_with = "prompt")]
        prompt_file: Option<PathBuf>,
        /// Clone the host's development database into the grove
        #[arg(long)]
        transplant: bool,
        /// Source database URL for --transplant (default: auto-detect from config/database.yml)
        #[arg(long, requires = "transplant")]
        db_source: Option<String>,
    },
    /// List all groves
    List,
    /// Show grove status and resource usage
    Status,
    /// Stop a grove (tear down containers/tmux but keep worktree and branch)
    Stop {
        /// Task name of the grove to stop
        task: String,
    },
    /// Start a stopped grove's containers
    Start {
        /// Task name of the grove to start
        task: String,
    },
    /// Uproot a grove and clean up all resources (worktree, branch, containers, tmux)
    Uproot {
        /// Task name of the grove to uproot
        task: String,
        /// Force uproot even if the worktree has uncommitted changes or unpushed commits
        #[arg(long)]
        force: bool,
    },
    /// Clean up orphaned groves
    Prune,
    /// Clone the host database into a running grove's compose stack
    Transplant {
        /// Task name of the grove
        task: String,
        /// Source database URL (default: auto-detect from config/database.yml)
        #[arg(long)]
        db_source: Option<String>,
    },
    /// Attach to a grove's tmux session (picks first if no task specified)
    Attach {
        /// Task name of the grove to attach to (optional — attaches to first grove if omitted)
        task: Option<String>,
    },
    /// Rebuild a grove's container image
    Build {
        /// Task name of the grove
        task: String,
    },
    /// Set tmux layout for grove panes
    Layout {
        /// Layout preset (tiled, even-horizontal, even-vertical, main-horizontal, main-vertical)
        preset: String,
    },
    /// Generate a default tmux-layout.json template
    InitTemplate,
    /// Generate a default claude-md.template for customization
    InitClaudeTemplate,
}

pub async fn run(cmd: GroveCommands) -> Result<()> {
    match cmd {
        GroveCommands::Plant {
            task,
            prompt,
            prompt_file,
            transplant,
            db_source,
        } => plant(&task, prompt, prompt_file, transplant, db_source).await,
        GroveCommands::List => list().await,
        GroveCommands::Status => status().await,
        GroveCommands::Stop { task } => stop(&task).await,
        GroveCommands::Start { task } => start(&task).await,
        GroveCommands::Uproot { task, force } => uproot(&task, force).await,
        GroveCommands::Prune => prune().await,
        GroveCommands::Transplant { task, db_source } => transplant(&task, db_source).await,
        GroveCommands::Attach { task } => attach(task.as_deref()).await,
        GroveCommands::Build { task } => build(&task).await,
        GroveCommands::Layout { preset } => set_layout(&preset).await,
        GroveCommands::InitTemplate => init_template().await,
        GroveCommands::InitClaudeTemplate => init_claude_template().await,
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
    prompt: Option<String>,
    prompt_file: Option<PathBuf>,
    db_clone: bool,
    db_source: Option<String>,
) -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    let local = LocalConfig::load(&treehouse_dir.join("local.yml"))?;
    let _ = cleanup_orphans(&treehouse_dir, &git);

    // Load task to get branch name
    let tasks_path = treehouse_dir.join("tasks.json");
    let contents = std::fs::read_to_string(&tasks_path)?;
    let mut tasks: Vec<task::Task> = serde_json::from_str(&contents)?;

    let task = tasks
        .iter_mut()
        .find(|t| t.name == task_name)
        .ok_or_else(|| TreehouseError::TaskNotFound(task_name.to_string()))?;

    if task.state == TaskState::Closed {
        return Err(TreehouseError::InvalidTaskState {
            current: task.state.to_string(),
            target: "plant grove".to_string(),
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

    let resolved_db_source = db_source.or(local.compose_db_source);

    // Plant the grove (always with compose)
    let state = orch_grove::plant(
        &git,
        &treehouse_dir,
        task_name,
        &branch_name,
        &local.tmux_session_name,
        local.min_disk_space_mb,
        initial_command.as_deref(),
        true, // always compose for grove
        local.compose_health_timeout_secs,
        &local.compose_post_start,
        db_clone,
        resolved_db_source.as_deref(),
    )?;

    println!(
        "{} Grove planted for task '{}'",
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
            style(format!("th grove attach {task_name}")).cyan()
        );
        let _ = ws;
    }

    Ok(())
}

async fn list() -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;
    let _ = cleanup_orphans(&treehouse_dir, &git);

    let groves = orch_grove::list_groves(&treehouse_dir)?;
    let groves: Vec<_> = groves.iter().filter(|g| g.compose_file.is_some()).collect();

    if groves.is_empty() {
        println!("No active groves.");
        return Ok(());
    }

    println!("{}", style("Active groves:").bold());
    for g in &groves {
        let worktree_ok = g.worktree_path.exists();
        let status = if worktree_ok {
            style("running").green()
        } else {
            style("orphaned").red()
        };

        let compose_info = if let Some(ref ports) = g.compose_ports {
            format!(" [compose: {}:{}:{}]", ports.app, ports.db, ports.redis)
        } else {
            String::new()
        };

        let session_info = g
            .tmux_session
            .as_ref()
            .map(|s| format!(" session:{s}"))
            .unwrap_or_default();

        println!(
            "  {} {} [{}] branch:{} worktree:{}{}{}",
            style("●").cyan(),
            g.task_name,
            status,
            g.branch,
            g.worktree_path.display(),
            session_info,
            compose_info
        );
    }

    Ok(())
}

async fn status() -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;
    let local = LocalConfig::load(&treehouse_dir.join("local.yml"))?;
    let _ = cleanup_orphans(&treehouse_dir, &git);

    let groves = orch_grove::list_groves(&treehouse_dir)?;
    let groves: Vec<_> = groves.iter().filter(|g| g.compose_file.is_some()).collect();

    println!("{}", style("Grove Status").bold());
    println!("Max environments: {}", local.max_workers);
    println!("Active groves: {}", groves.len());
    println!();

    if groves.is_empty() {
        println!("No active groves.");
    } else {
        for g in &groves {
            let age = chrono::Utc::now() - g.created_at;
            let hours = age.num_hours();
            let mins = age.num_minutes() % 60;

            let session_status = g.tmux_session.as_ref().map(|ws| {
                if session::session_exists(ws) {
                    style("active").green()
                } else {
                    style("inactive").red()
                }
            });

            println!(
                "  {} {} (uptime: {}h {}m)",
                style("●").green(),
                g.task_name,
                hours,
                mins
            );
            println!("    Branch:   {}", g.branch);
            println!("    Worktree: {}", g.worktree_path.display());
            if let Some(ref ws) = g.tmux_session {
                println!("    Session:  {ws} [{}]", session_status.unwrap());
            }
            if let Some(ref ports) = g.compose_ports {
                println!(
                    "    Compose:  app:{} db:{} redis:{}",
                    ports.app, ports.db, ports.redis
                );
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
        "{} Grove '{}' stopped (containers/tmux removed, worktree and branch preserved)",
        style("✓").green().bold(),
        task_name
    );
    println!(
        "  Re-plant with: {}",
        style(format!("th grove plant {task_name}")).cyan()
    );

    Ok(())
}

async fn start(task_name: &str) -> Result<()> {
    let docker = DockerClient::connect().await?;

    let container_name = format!("treehouse-{task_name}");
    let image = format!("treehouse-{task_name}:latest");

    if docker.container_exists(&container_name).await {
        println!("Container '{container_name}' already exists. Stopping first...");
        let _ = docker.stop_container(&container_name).await;
        docker.remove_container(&container_name).await?;
    }

    let id = docker
        .create_and_start_container(&container_name, &image, "/app", ".")
        .await?;

    println!(
        "{} Container '{}' started ({})",
        style("✓").green().bold(),
        container_name,
        &id[..12]
    );
    Ok(())
}

async fn uproot(task_name: &str, force: bool) -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    orch_grove::uproot(&git, &treehouse_dir, task_name, force)?;

    println!(
        "{} Grove '{}' uprooted and resources cleaned up",
        style("✓").green().bold(),
        task_name
    );

    Ok(())
}

async fn prune() -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    let orphans = cleanup::find_orphans(&treehouse_dir)?;

    if orphans.is_empty() {
        println!("No orphaned groves found.");
        return Ok(());
    }

    println!(
        "{} Found {} orphaned grove(s):",
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
        cleanup::cleanup_orphan(&treehouse_dir, &git.root, o)?;
    }

    println!(
        "{} Pruned {} orphaned grove(s)",
        style("✓").green().bold(),
        orphans.len()
    );

    Ok(())
}

async fn transplant(task_name: &str, source: Option<String>) -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;
    let local = LocalConfig::load(&treehouse_dir.join("local.yml"))?;

    // Load grove state
    let state_path = GroveState::state_path(&treehouse_dir, task_name);
    if !state_path.exists() {
        return Err(TreehouseError::GroveNotFound(task_name.to_string()));
    }
    let state = GroveState::load(&state_path)?;

    // Verify grove has a compose stack
    let compose_file = state.compose_file.as_ref().ok_or_else(|| {
        TreehouseError::Other(format!(
            "Grove '{task_name}' has no compose stack. \
             Database transplanting requires a compose stack."
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
        "{} Database transplanted into grove '{task_name}'",
        style("✓").green().bold(),
    );

    Ok(())
}

async fn attach(task_name: Option<&str>) -> Result<()> {
    if !session::is_available() {
        return Err(TreehouseError::TmuxNotAvailable);
    }

    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;
    let groves = orch_grove::list_groves(&treehouse_dir)?;

    if groves.is_empty() {
        println!("No active groves. Plant a grove first.");
        return Ok(());
    }

    if let Some(name) = task_name {
        let grove = groves
            .iter()
            .find(|g| g.task_name == name)
            .ok_or_else(|| TreehouseError::GroveNotFound(name.to_string()))?;

        let ws_name = grove.tmux_session.as_ref().ok_or_else(|| {
            TreehouseError::Other(format!("Grove '{name}' has no tmux session"))
        })?;

        if session::session_exists(ws_name) {
            session::attach_session(ws_name)?;
        } else {
            println!("Session '{ws_name}' no longer exists.");
        }
        return Ok(());
    }

    // No task specified — attach to first grove's session
    for g in &groves {
        if let Some(ref ws_name) = g.tmux_session {
            if session::session_exists(ws_name) {
                session::attach_session(ws_name)?;
                return Ok(());
            }
        }
    }

    println!("No active grove sessions found.");
    Ok(())
}

async fn build(task_name: &str) -> Result<()> {
    let docker = DockerClient::connect().await?;

    let tag = format!("treehouse-{task_name}:latest");
    println!("Building image '{tag}'...");

    let dockerfile = "FROM ubuntu:22.04\nRUN apt-get update -qq\nCMD [\"sleep\", \"infinity\"]\n";
    docker.build_image(dockerfile, &tag).await?;

    println!("{} Image '{}' built", style("✓").green().bold(), tag);
    Ok(())
}

async fn set_layout(preset: &str) -> Result<()> {
    if !session::is_available() {
        return Err(TreehouseError::TmuxNotAvailable);
    }

    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;
    let local = LocalConfig::load(&treehouse_dir.join("local.yml"))?;
    layout::apply_layout(&local.tmux_session_name, preset)?;

    println!(
        "{} Applied layout '{}'",
        style("✓").green().bold(),
        preset,
    );
    Ok(())
}

async fn init_template() -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    let path = treehouse_dir.join("tmux-layout.json");
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
    println!("  Edit it to customize your per-grove workspace layout.");

    Ok(())
}

async fn init_claude_template() -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = ensure_treehouse(&git)?;

    let path = treehouse_dir.join("claude-md.template");
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
    println!("  Edit it to customize the CLAUDE.local.md generated in each grove's worktree.");
    println!("  Available variables: {{{{WORKTREE_PATH}}}}, {{{{WORKER_NAME}}}}, {{{{BRANCH_NAME}}}},");
    println!("    {{{{PROJECT_NAME}}}}, {{{{TASK_TYPE}}}}, {{{{DETECTED_TYPES}}}}, {{{{COMPOSE_FILE}}}},");
    println!("    {{{{COMPOSE_PROJECT}}}}, {{{{APP_PORT}}}}, {{{{DB_PORT}}}}, {{{{REDIS_PORT}}}}");
    println!("  Conditionals: {{{{#if COMPOSE_ENABLED}}}}...{{{{/if}}}}, {{{{#if !COMPOSE_ENABLED}}}}...{{{{/if}}}}");

    Ok(())
}

fn cleanup_orphans(
    treehouse_dir: &std::path::Path,
    git: &GitRepo,
) -> Result<()> {
    let orphans = cleanup::find_orphans(treehouse_dir)?;
    for orphan in &orphans {
        cleanup::cleanup_orphan(treehouse_dir, &git.root, orphan)?;
    }
    if !orphans.is_empty() {
        println!(
            "{} Pruned {} orphaned grove(s)",
            style("!").yellow(),
            orphans.len()
        );
    }
    Ok(())
}
