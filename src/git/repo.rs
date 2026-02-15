use std::path::{Path, PathBuf};

use git2::Repository;

use crate::error::{TreehouseError, Result};

pub struct GitRepo {
    pub repo: Repository,
    pub root: PathBuf,
}

impl GitRepo {
    pub fn discover() -> Result<Self> {
        let repo = Repository::discover(".").map_err(|_| TreehouseError::NotGitRepo)?;
        let root = Self::resolve_root(&repo)?;
        Ok(Self { repo, root })
    }

    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::open(path).map_err(|_| TreehouseError::NotGitRepo)?;
        let root = Self::resolve_root(&repo)?;
        Ok(Self { repo, root })
    }

    /// Resolve the main repo root, even when called from inside a worktree.
    /// `repo.workdir()` returns the worktree's own directory, so we use
    /// `repo.commondir()` (points to the real `.git`) and go up one level.
    fn resolve_root(repo: &Repository) -> Result<PathBuf> {
        let commondir = repo.commondir().to_path_buf();
        // commondir is e.g. `/repo/.git` — parent is the repo root
        let root = commondir
            .parent()
            .ok_or(TreehouseError::NotGitRepo)?
            .to_path_buf();
        Ok(root)
    }

    pub fn head_commit_id(&self) -> Result<git2::Oid> {
        let head = self.repo.head()?;
        Ok(head.peel_to_commit()?.id())
    }

    pub fn treehouse_dir(&self) -> PathBuf {
        self.root.join(".treehouse")
    }
}
