use crate::error::{TreehouseError, Result};

use super::repo::GitRepo;

/// Format a branch name following the convention: <project>/<type>/<name>
pub fn format_branch_name(project: &str, task_type: &str, name: &str) -> String {
    let sanitized_name = name
        .to_lowercase()
        .replace(' ', "-")
        .replace(|c: char| !c.is_alphanumeric() && c != '-', "");
    format!("{project}/{task_type}/{sanitized_name}")
}

/// Create a new branch from HEAD
pub fn create_branch(git: &GitRepo, branch_name: &str) -> Result<()> {
    let head = git.repo.head()?;
    let commit = head.peel_to_commit()?;

    if git.repo.find_branch(branch_name, git2::BranchType::Local).is_ok() {
        return Err(TreehouseError::BranchAlreadyExists(branch_name.to_string()));
    }

    git.repo.branch(branch_name, &commit, false)?;
    Ok(())
}

/// Delete a local branch
pub fn delete_branch(git: &GitRepo, branch_name: &str) -> Result<()> {
    let mut branch = git
        .repo
        .find_branch(branch_name, git2::BranchType::Local)
        .map_err(|_| TreehouseError::Other(format!("Branch not found: {branch_name}")))?;
    branch.delete()?;
    Ok(())
}

/// Check if a branch exists
pub fn branch_exists(git: &GitRepo, branch_name: &str) -> bool {
    git.repo
        .find_branch(branch_name, git2::BranchType::Local)
        .is_ok()
}
