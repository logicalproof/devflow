use std::path::Path;

use chrono::{DateTime, Utc};
use clap::Subcommand;
use console::style;
use serde::{Deserialize, Serialize};

use crate::config::project::ProjectConfig;
use crate::error::{DevflowError, Result};
use crate::git::branch;
use crate::git::repo::GitRepo;

#[derive(Subcommand)]
pub enum TaskCommands {
    /// Create a new task
    Create {
        /// Task name (used as identifier)
        name: String,
        /// Task type (feature, bugfix, refactor, chore)
        #[arg(short = 't', long, default_value = "feature")]
        task_type: String,
        /// Description of the task
        #[arg(short, long)]
        description: Option<String>,
    },
    /// List all tasks
    List,
    /// Show task details
    Show {
        /// Task name
        name: String,
    },
    /// Pause an active task
    Pause {
        /// Task name
        name: String,
    },
    /// Resume a paused task
    Resume {
        /// Task name
        name: String,
    },
    /// Mark task as completed
    Complete {
        /// Task name
        name: String,
    },
    /// Close a completed task (cleanup branches/worktrees)
    Close {
        /// Task name
        name: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Task {
    pub name: String,
    pub task_type: String,
    pub description: String,
    pub state: TaskState,
    pub branch: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Created,
    Active,
    Paused,
    Completed,
    Closed,
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskState::Created => write!(f, "created"),
            TaskState::Active => write!(f, "active"),
            TaskState::Paused => write!(f, "paused"),
            TaskState::Completed => write!(f, "completed"),
            TaskState::Closed => write!(f, "closed"),
        }
    }
}

fn tasks_path(devflow_dir: &Path) -> std::path::PathBuf {
    devflow_dir.join("tasks.json")
}

fn load_tasks(devflow_dir: &Path) -> Result<Vec<Task>> {
    let path = tasks_path(devflow_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read_to_string(&path)?;
    let tasks: Vec<Task> = serde_json::from_str(&contents)?;
    Ok(tasks)
}

fn save_tasks(devflow_dir: &Path, tasks: &[Task]) -> Result<()> {
    let path = tasks_path(devflow_dir);
    let contents = serde_json::to_string_pretty(tasks)?;
    std::fs::write(path, contents)?;
    Ok(())
}

fn find_task_mut<'a>(tasks: &'a mut [Task], name: &str) -> Result<&'a mut Task> {
    tasks
        .iter_mut()
        .find(|t| t.name == name)
        .ok_or_else(|| DevflowError::TaskNotFound(name.to_string()))
}

fn ensure_devflow(git: &GitRepo) -> Result<std::path::PathBuf> {
    let devflow_dir = git.devflow_dir();
    if !devflow_dir.join("config.yml").exists() {
        return Err(DevflowError::NotInitialized);
    }
    Ok(devflow_dir)
}

pub async fn run(cmd: TaskCommands) -> Result<()> {
    match cmd {
        TaskCommands::Create {
            name,
            task_type,
            description,
        } => create(&name, &task_type, description.as_deref()).await,
        TaskCommands::List => list().await,
        TaskCommands::Show { name } => show(&name).await,
        TaskCommands::Pause { name } => transition(&name, TaskState::Active, TaskState::Paused).await,
        TaskCommands::Resume { name } => transition(&name, TaskState::Paused, TaskState::Active).await,
        TaskCommands::Complete { name } => transition(&name, TaskState::Active, TaskState::Completed).await,
        TaskCommands::Close { name } => close(&name).await,
    }
}

async fn create(name: &str, task_type: &str, description: Option<&str>) -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;
    let mut tasks = load_tasks(&devflow_dir)?;

    // Check for duplicate
    if tasks.iter().any(|t| t.name == name) {
        return Err(DevflowError::TaskAlreadyExists(name.to_string()));
    }

    // Load project config for branch naming
    let config = ProjectConfig::load(&devflow_dir.join("config.yml"))?;
    let branch_name = branch::format_branch_name(&config.project_name, task_type, name);

    // Create the branch
    branch::create_branch(&git, &branch_name)?;

    let now = Utc::now();
    let task = Task {
        name: name.to_string(),
        task_type: task_type.to_string(),
        description: description.unwrap_or("").to_string(),
        state: TaskState::Created,
        branch: branch_name.clone(),
        created_at: now,
        updated_at: now,
    };

    tasks.push(task);
    save_tasks(&devflow_dir, &tasks)?;

    println!(
        "{} Created task '{}' with branch '{}'",
        style("✓").green().bold(),
        name,
        branch_name
    );

    Ok(())
}

async fn list() -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;
    let tasks = load_tasks(&devflow_dir)?;

    if tasks.is_empty() {
        println!("No tasks. Create one with: devflow task create <name>");
        return Ok(());
    }

    println!("{}", style("Tasks:").bold());
    for t in &tasks {
        let state_str = t.state.to_string();
        let state_style = match t.state {
            TaskState::Created => style(&state_str).cyan(),
            TaskState::Active => style(&state_str).green(),
            TaskState::Paused => style(&state_str).yellow(),
            TaskState::Completed => style(&state_str).blue(),
            TaskState::Closed => style(&state_str).dim(),
        };
        println!(
            "  {} {} [{}] ({})",
            style("●").cyan(),
            t.name,
            state_style,
            t.branch
        );
    }

    Ok(())
}

async fn show(name: &str) -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;
    let tasks = load_tasks(&devflow_dir)?;

    let task = tasks
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| DevflowError::TaskNotFound(name.to_string()))?;

    println!("{}", style(&task.name).bold());
    println!("  Type:        {}", task.task_type);
    println!("  State:       {}", task.state);
    println!("  Branch:      {}", task.branch);
    println!("  Description: {}", if task.description.is_empty() { "(none)" } else { &task.description });
    println!("  Created:     {}", task.created_at.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("  Updated:     {}", task.updated_at.format("%Y-%m-%d %H:%M:%S UTC"));

    Ok(())
}

async fn transition(name: &str, expected: TaskState, target: TaskState) -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;
    let mut tasks = load_tasks(&devflow_dir)?;

    let task = find_task_mut(&mut tasks, name)?;

    if task.state != expected {
        return Err(DevflowError::InvalidTaskState {
            current: task.state.to_string(),
            target: target.to_string(),
        });
    }

    task.state = target.clone();
    task.updated_at = Utc::now();
    save_tasks(&devflow_dir, &tasks)?;

    println!(
        "{} Task '{}' is now {}",
        style("✓").green().bold(),
        name,
        target
    );

    Ok(())
}

async fn close(name: &str) -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = ensure_devflow(&git)?;
    let mut tasks = load_tasks(&devflow_dir)?;

    let task = find_task_mut(&mut tasks, name)?;

    if task.state == TaskState::Closed {
        return Err(DevflowError::InvalidTaskState {
            current: task.state.to_string(),
            target: "closed".to_string(),
        });
    }

    // Clean up branch
    let _ = branch::delete_branch(&git, &task.branch);

    // Clean up worktree if it exists
    let worktree_path = devflow_dir.join("worktrees").join(name);
    if worktree_path.exists() {
        let _ = crate::git::worktree::remove_worktree(&git.root, &worktree_path);
    }

    task.state = TaskState::Closed;
    task.updated_at = Utc::now();
    save_tasks(&devflow_dir, &tasks)?;

    println!(
        "{} Task '{}' closed and resources cleaned up",
        style("✓").green().bold(),
        name
    );

    Ok(())
}
