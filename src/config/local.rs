use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct LocalConfig {
    #[serde(default)]
    pub tmux_session_name: String,
    #[serde(default)]
    pub max_workers: usize,
    #[serde(default)]
    pub min_disk_space_mb: u64,
}

impl LocalConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)?;
        let config: Self = serde_yml::from_str(&contents)?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let contents = serde_yml::to_string(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    pub fn with_defaults() -> Self {
        Self {
            tmux_session_name: "devflow".to_string(),
            max_workers: 4,
            min_disk_space_mb: 500,
        }
    }
}
