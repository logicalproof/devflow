use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    pub project_name: String,
    #[serde(default)]
    pub detected_types: Vec<String>,
    #[serde(default)]
    pub container_enabled: bool,
    #[serde(default)]
    pub default_branch: String,
}

impl ProjectConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: Self = serde_yml::from_str(&contents)?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let contents = serde_yml::to_string(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }
}
