use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::compose::ports::AllocatedPorts;
use crate::error::Result;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkerState {
    pub task_name: String,
    pub branch: String,
    pub worktree_path: PathBuf,
    pub tmux_window: String,
    pub container_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub pid: Option<u32>,
    #[serde(default)]
    pub compose_file: Option<PathBuf>,
    #[serde(default)]
    pub compose_ports: Option<AllocatedPorts>,
    #[serde(default)]
    pub tmux_session: Option<String>,
}

impl WorkerState {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let state: Self = serde_json::from_str(&contents)?;
        Ok(state)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    pub fn state_path(devflow_dir: &Path, task_name: &str) -> PathBuf {
        devflow_dir.join("workers").join(format!("{task_name}.json"))
    }
}
