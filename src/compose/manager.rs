use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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

    // Auto-extract ARG directives from Dockerfile and inject as build args
    let dockerfile_path = worktree_path.join("Dockerfile.devflow");
    let build_args = template::extract_dockerfile_args(&dockerfile_path);
    let rendered = template::inject_build_args(&rendered, &build_args);

    let compose_dir = devflow_dir.join("compose").join(worker_name);
    std::fs::create_dir_all(&compose_dir)?;

    let compose_file = compose_dir.join("docker-compose.yml");
    std::fs::write(&compose_file, rendered)?;

    // Copy .env into compose directory so Docker Compose can resolve ${VAR} in the template
    let worktree_env = worktree_path.join(".env");
    if worktree_env.exists() {
        let _ = std::fs::copy(&worktree_env, compose_dir.join(".env"));
    }

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

/// Wait for all services in the compose stack to be running (and healthy, if
/// a healthcheck is defined). Polls `docker compose ps --format json` every 2s.
pub fn wait_healthy(compose_file: &Path, timeout: Duration) -> Result<()> {
    let project = project_name(compose_file);
    let start = Instant::now();

    println!("Waiting for containers to be ready...");

    loop {
        let output = Command::new("docker")
            .args([
                "compose",
                "-f",
                &compose_file.to_string_lossy(),
                "-p",
                &project,
                "ps",
                "--format",
                "json",
            ])
            .output()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut total = 0u32;
            let mut ready = 0u32;

            for line in stdout.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(svc) = serde_json::from_str::<serde_json::Value>(line) {
                    total += 1;
                    let state = svc
                        .get("State")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let health = svc
                        .get("Health")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let name = svc
                        .get("Name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");

                    // Fail fast if a container has exited or died
                    if state == "exited" || state == "dead" {
                        return Err(DevflowError::ComposeOperationFailed(format!(
                            "container '{name}' {state} unexpectedly"
                        )));
                    }

                    // Running with no healthcheck, or running+healthy
                    if state == "running" && (health.is_empty() || health == "healthy") {
                        ready += 1;
                    }
                }
            }

            if total > 0 && ready == total {
                println!("  All {total} container(s) ready.");
                return Ok(());
            }

            println!("  {ready}/{total} container(s) ready...");
        }

        if start.elapsed() >= timeout {
            return Err(DevflowError::ComposeOperationFailed(format!(
                "containers not ready after {}s",
                timeout.as_secs()
            )));
        }

        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Execute a command inside a running compose service (non-interactive).
pub fn exec(compose_file: &Path, service: &str, cmd: &str) -> Result<()> {
    let project = project_name(compose_file);
    let output = Command::new("docker")
        .args([
            "compose",
            "-f",
            &compose_file.to_string_lossy(),
            "-p",
            &project,
            "exec",
            "-T",
            service,
            "sh",
            "-c",
            cmd,
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        print!("{stdout}");
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::ComposeOperationFailed(format!(
            "exec '{cmd}' failed: {stderr}"
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
