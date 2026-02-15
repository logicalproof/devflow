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
        let root = repo
            .workdir()
            .ok_or(TreehouseError::NotGitRepo)?
            .to_path_buf();
        Ok(Self { repo, root })
    }

    pub fn open(path: &Path) -> Result<Self> {
        let repo = Repository::open(path).map_err(|_| TreehouseError::NotGitRepo)?;
        let root = repo
            .workdir()
            .ok_or(TreehouseError::NotGitRepo)?
            .to_path_buf();
        Ok(Self { repo, root })
    }

    pub fn head_commit_id(&self) -> Result<git2::Oid> {
        let head = self.repo.head()?;
        Ok(head.peel_to_commit()?.id())
    }

    pub fn treehouse_dir(&self) -> PathBuf {
        self.root.join(".treehouse")
    }
}
