use clap::Subcommand;
use console::style;

use crate::error::Result;
use crate::git::repo::GitRepo;
use crate::git::worktree as wt;

#[derive(Subcommand)]
pub enum WorktreeCommands {
    /// List all worktrees
    List,
    /// Remove stale worktrees
    Prune,
    /// Check worktree health
    Health,
}

pub async fn run(cmd: WorktreeCommands) -> Result<()> {
    match cmd {
        WorktreeCommands::List => list().await,
        WorktreeCommands::Prune => prune().await,
        WorktreeCommands::Health => health().await,
    }
}

async fn list() -> Result<()> {
    let git = GitRepo::discover()?;
    let worktrees = wt::list_worktrees(&git.root)?;

    if worktrees.is_empty() {
        println!("No worktrees found.");
        return Ok(());
    }

    println!("{}", style("Worktrees:").bold());
    for w in &worktrees {
        let branch_str = w.branch.as_deref().unwrap_or("(detached)");
        let status = if w.path.exists() {
            style("ok").green()
        } else {
            style("missing").red()
        };
        println!(
            "  {} {} [{}] ({})",
            style("●").cyan(),
            w.path.display(),
            branch_str,
            status
        );
    }

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
            "\n{healthy} healthy, {unhealthy} unhealthy. Run 'devflow worktree prune' to clean up."
        );
    }

    Ok(())
}
