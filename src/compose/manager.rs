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
    let (tmpl, is_custom) = template::load_or_default(devflow_dir)?;

    if is_custom {
        println!(
            "  Using custom compose template: {}",
            devflow_dir.join("compose-template.yml").display()
        );
    }

    let vars = TemplateVars {
        worker_name,
        worktree_path: &worktree_path.to_string_lossy(),
        ports,
    };
    let rendered = template::render(&tmpl, &vars);

    // Auto-extract ARG directives from Dockerfile, only for vars defined in .env
    let dockerfile_path = worktree_path.join("Dockerfile.devflow");
    let env_path = worktree_path.join(".env");
    let build_args = template::extract_dockerfile_args(&dockerfile_path, &env_path);

    if !build_args.is_empty() {
        println!("  Injecting build args from Dockerfile + .env: {}", build_args.join(", "));
    }

    let (rendered, injected) = template::inject_build_args(&rendered, &build_args);

    if !build_args.is_empty() && !injected {
        eprintln!(
            "Warning: Found build args {:?} but could not inject into compose template \
             (no 'dockerfile:' or 'context:' line found). \
             Add them manually to your compose-template.yml under build: args:",
            build_args
        );
    }

    // Auto-detect multi-stage Dockerfile and target the build stage for dev
    let rendered = if let Some(target) = template::detect_build_target(&dockerfile_path) {
        println!("  Multi-stage Dockerfile detected, targeting '{target}' stage");
        template::inject_build_target(&rendered, &target)
    } else {
        rendered
    };

    // Auto-extract build secrets (RUN --mount=type=secret) from Dockerfile
    let build_secrets = template::extract_dockerfile_secrets(&dockerfile_path, &env_path);
    let rendered = if !build_secrets.is_empty() {
        println!(
            "  Injecting build secrets from Dockerfile + .env: {}",
            build_secrets.join(", ")
        );
        template::inject_build_secrets(&rendered, &build_secrets)
    } else {
        rendered
    };

    let compose_dir = devflow_dir.join("compose").join(worker_name);
    std::fs::create_dir_all(&compose_dir)?;

    let compose_file = compose_dir.join("docker-compose.yml");
    std::fs::write(&compose_file, &rendered)?;

    // Copy .env into compose directory, normalizing for Docker Compose compatibility.
    // Docker Compose .env does NOT support `export` prefix or quoted values.
    let worktree_env = worktree_path.join(".env");
    if worktree_env.exists() {
        match normalize_env_file(&worktree_env, &compose_dir.join(".env")) {
            Ok(_) => println!("  Copied .env to compose directory (normalized)"),
            Err(e) => eprintln!("Warning: failed to copy .env to compose directory: {e}"),
        }
    } else {
        eprintln!(
            "Warning: no .env found at {} — build args may not resolve",
            worktree_env.display()
        );
    }

    Ok(compose_file)
}

/// Start the compose stack in detached mode.
pub fn up(compose_file: &Path) -> Result<()> {
    let project = project_name(compose_file);
    let compose_dir = compose_file.parent().unwrap_or(Path::new("."));
    let env_file = compose_dir.join(".env");

    let mut args = vec![
        "compose".to_string(),
        "-f".to_string(),
        compose_file.to_string_lossy().to_string(),
        "-p".to_string(),
        project,
    ];

    // Explicitly point Docker Compose to the .env file for variable substitution
    if env_file.exists() {
        args.push("--env-file".to_string());
        args.push(env_file.to_string_lossy().to_string());
    }

    args.extend([
        "up".to_string(),
        "-d".to_string(),
        "--build".to_string(),
    ]);

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut cmd = Command::new("docker");
    cmd.args(&arg_refs);

    // Load .env vars into the process environment so Docker Compose
    // can use them for both ${VAR} substitution AND secrets (environment: VAR).
    if env_file.exists() {
        if let Ok(contents) = std::fs::read_to_string(&env_file) {
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = trimmed.split_once('=') {
                    cmd.env(key.trim(), value.trim());
                }
            }
        }
    }

    let output = cmd.output()?;

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
    exec_as_user(compose_file, service, cmd, None)
}

/// Execute a command inside a running compose service as a specific user.
pub fn exec_as_user(compose_file: &Path, service: &str, cmd: &str, user: Option<&str>) -> Result<()> {
    let project = project_name(compose_file);
    let compose_path = compose_file.to_string_lossy();
    let mut args = vec![
        "compose",
        "-f",
        &compose_path,
        "-p",
        &project,
        "exec",
        "-T",
    ];
    if let Some(u) = user {
        args.push("--user");
        args.push(u);
    }
    args.extend([service, "sh", "-c", cmd]);

    let output = Command::new("docker")
        .args(&args)
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

/// Normalize a .env file for Docker Compose compatibility.
/// Strips `export` prefixes and surrounding quotes from values.
fn normalize_env_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    let contents = std::fs::read_to_string(src)?;
    let mut normalized = String::new();

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            normalized.push_str(line);
            normalized.push('\n');
            continue;
        }

        // Strip `export ` prefix
        let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);

        // Split into key=value, strip quotes from value
        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            // Strip surrounding single or double quotes
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| {
                    value
                        .strip_prefix('\'')
                        .and_then(|v| v.strip_suffix('\''))
                })
                .unwrap_or(value);
            normalized.push_str(&format!("{key}={value}\n"));
        } else {
            // Key with no value
            normalized.push_str(trimmed);
            normalized.push('\n');
        }
    }

    std::fs::write(dst, normalized)
}

/// Derive the project name from the compose file's parent directory.
pub fn project_name(compose_file: &Path) -> String {
    compose_file
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| format!("devflow-{}", n.to_string_lossy()))
        .unwrap_or_else(|| "devflow".to_string())
}
