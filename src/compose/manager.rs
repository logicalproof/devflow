use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{DevflowError, Result};

use super::ports::AllocatedPorts;
use super::template::{self, TemplateVars};

/// Check that `docker compose` is available on the system.
pub fn check_available() -> Result<()> {
    let output = Command::new("docker")
        .args(["compose", "version"])
        .output()
        .map_err(|_| DevflowError::ComposeNotAvailable)?;

    if !output.status.success() {
        return Err(DevflowError::ComposeNotAvailable);
    }
    Ok(())
}

/// Generate a docker-compose.yml for a worker from the template.
pub fn generate_compose_file(
    devflow_dir: &Path,
    worker_name: &str,
    worktree_path: &Path,
    ports: &AllocatedPorts,
) -> Result<PathBuf> {
    let tmpl = template::load_or_default(devflow_dir)?;

    let vars = TemplateVars {
        worker_name,
        worktree_path: &worktree_path.to_string_lossy(),
        ports,
    };
    let rendered = template::render(&tmpl, &vars);

    let compose_dir = devflow_dir.join("compose").join(worker_name);
    std::fs::create_dir_all(&compose_dir)?;

    let compose_file = compose_dir.join("docker-compose.yml");
    std::fs::write(&compose_file, rendered)?;

    Ok(compose_file)
}

/// Start the compose stack in detached mode.
pub fn up(compose_file: &Path) -> Result<()> {
    let project = project_name(compose_file);
    let output = Command::new("docker")
        .args([
            "compose",
            "-f",
            &compose_file.to_string_lossy(),
            "-p",
            &project,
            "up",
            "-d",
            "--build",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::ComposeOperationFailed(format!(
            "compose up failed: {stderr}"
        )));
    }
    Ok(())
}

/// Tear down the compose stack and remove volumes.
pub fn down(compose_file: &Path) -> Result<()> {
    let project = project_name(compose_file);
    let output = Command::new("docker")
        .args([
            "compose",
            "-f",
            &compose_file.to_string_lossy(),
            "-p",
            &project,
            "down",
            "-v",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::ComposeOperationFailed(format!(
            "compose down failed: {stderr}"
        )));
    }
    Ok(())
}

/// Derive the project name from the compose file's parent directory.
fn project_name(compose_file: &Path) -> String {
    compose_file
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| format!("devflow-{}", n.to_string_lossy()))
        .unwrap_or_else(|| "devflow".to_string())
}
