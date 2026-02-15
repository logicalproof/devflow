use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{DevflowError, Result};

/// Create a new worktree at the given path for the given branch (shells out to git CLI)
pub fn create_worktree(repo_root: &Path, worktree_path: &Path, branch: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "add", "--quiet"])
        .arg(worktree_path)
        .arg(branch)
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::GitCommand(format!(
            "Failed to create worktree: {stderr}"
        )));
    }
    Ok(())
}

/// Remove a worktree (shells out to git CLI)
pub fn remove_worktree(repo_root: &Path, worktree_path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(worktree_path)
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::GitCommand(format!(
            "Failed to remove worktree: {stderr}"
        )));
    }
    Ok(())
}

/// List worktrees (shells out to git CLI)
pub fn list_worktrees(repo_root: &Path) -> Result<Vec<WorktreeInfo>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::GitCommand(format!(
            "Failed to list worktrees: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_worktree_list(&stdout))
}

/// Prune stale worktree entries
pub fn prune_worktrees(repo_root: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::GitCommand(format!(
            "Failed to prune worktrees: {stderr}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub bare: bool,
}

fn parse_worktree_list(output: &str) -> Vec<WorktreeInfo> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head = String::new();
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            // Save previous entry
            if let Some(prev_path) = current_path.take() {
                worktrees.push(WorktreeInfo {
                    path: prev_path,
                    head: std::mem::take(&mut current_head),
                    branch: current_branch.take(),
                    bare: is_bare,
                });
                is_bare = false;
            }
            current_path = Some(PathBuf::from(path));
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            current_head = head.to_string();
        } else if let Some(branch) = line.strip_prefix("branch ") {
            current_branch = Some(branch.replace("refs/heads/", ""));
        } else if line == "bare" {
            is_bare = true;
        }
    }

    // Don't forget the last entry
    if let Some(path) = current_path {
        worktrees.push(WorktreeInfo {
            path,
            head: current_head,
            branch: current_branch,
            bare: is_bare,
        });
    }

    worktrees
}

/// Check if a worktree path exists and is valid
pub fn worktree_exists(path: &Path) -> bool {
    path.exists() && path.join(".git").exists()
}

/// Check if a worktree has uncommitted changes (staged, unstaged, or untracked)
pub fn has_uncommitted_changes(worktree_path: &Path) -> bool {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(o) if o.status.success() => !o.stdout.is_empty(),
        _ => false,
    }
}

/// Count commits on `branch` that are not on `base_branch`.
/// Returns 0 on any error (non-fatal usage).
pub fn commits_ahead_of(repo_root: &Path, branch: &str, base_branch: &str) -> u64 {
    let range = format!("{base_branch}...{branch}");
    let output = Command::new("git")
        .args(["rev-list", "--count", &range])
        .current_dir(repo_root)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse()
                .unwrap_or(0)
        }
        _ => 0,
    }
}
