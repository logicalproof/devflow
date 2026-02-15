use std::path::Path;

use crate::error::Result;

use super::ports::AllocatedPorts;

pub struct TemplateVars<'a> {
    pub worker_name: &'a str,
    pub worktree_path: &'a str,
    pub ports: &'a AllocatedPorts,
}

/// Load a user-provided compose template, or fall back to the built-in default.
/// Returns (template_content, is_custom).
pub fn load_or_default(devflow_dir: &Path) -> Result<(String, bool)> {
    let custom_path = devflow_dir.join("compose-template.yml");
    if custom_path.exists() {
        let contents = std::fs::read_to_string(&custom_path)?;
        return Ok((contents, true));
    }
    Ok((default_rails_template().to_string(), false))
}

/// Render template variables using simple string replacement.
pub fn render(template: &str, vars: &TemplateVars) -> String {
    template
        .replace("{{WORKER_NAME}}", vars.worker_name)
        .replace("{{APP_PORT}}", &vars.ports.app.to_string())
        .replace("{{DB_PORT}}", &vars.ports.db.to_string())
        .replace("{{REDIS_PORT}}", &vars.ports.redis.to_string())
        .replace("{{WORKTREE_PATH}}", vars.worktree_path)
}

/// Extract ARG names from a Dockerfile that have a matching key in the .env file.
/// Only includes ARGs where the .env provides a value, so Dockerfile defaults are preserved.
pub fn extract_dockerfile_args(dockerfile_path: &Path, env_path: &Path) -> Vec<String> {
    let contents = match std::fs::read_to_string(dockerfile_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    // Parse .env file to find which vars are defined (handles `export` prefix and quotes)
    let env_keys: std::collections::HashSet<String> = std::fs::read_to_string(env_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            trimmed.split('=').next().map(|k| k.trim().to_string())
        })
        .collect();

    contents
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("ARG ") {
                let rest = trimmed.strip_prefix("ARG ").unwrap().trim();
                let name = rest.split(['=', ' ']).next().unwrap_or("").trim();
                if !name.is_empty() && env_keys.contains(name) {
                    Some(name.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
}

/// Inject build args into a rendered compose file using ${VAR} substitution.
/// Docker Compose resolves these from the normalized .env in the compose directory.
/// Returns (rendered_content, was_injected).
pub fn inject_build_args(rendered: &str, args: &[String]) -> (String, bool) {
    if args.is_empty() {
        return (rendered.to_string(), true); // nothing to inject = success
    }

    // Check if there's already an `args:` section under `build:`
    let has_existing_args = rendered.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "args:" && line.starts_with("      ")
    });

    if has_existing_args {
        return inject_into_existing_args(rendered, args);
    }

    // Try injecting after `dockerfile:` line
    if let Some(result) = try_inject_after(rendered, "dockerfile:", args) {
        return (result, true);
    }

    // Fallback: try injecting after `context:` line
    if let Some(result) = try_inject_after(rendered, "context:", args) {
        return (result, true);
    }

    (rendered.to_string(), false)
}

/// Try injecting args after the first line matching `marker`.
fn try_inject_after(rendered: &str, marker: &str, args: &[String]) -> Option<String> {
    let mut result = String::new();
    let mut injected = false;

    for line in rendered.lines() {
        result.push_str(line);
        result.push('\n');

        if !injected && line.trim().starts_with(marker) {
            let indent = &line[..line.len() - line.trim_start().len()];
            result.push_str(indent);
            result.push_str("args:\n");
            for arg in args {
                result.push_str(indent);
                result.push_str(&format!("  - {arg}=${{{arg}}}\n"));
            }
            injected = true;
        }
    }

    injected.then_some(result)
}

/// Append args to an existing `args:` section, skipping any already present.
fn inject_into_existing_args(rendered: &str, args: &[String]) -> (String, bool) {
    let mut result = String::new();
    let mut injected = false;
    let lines: Vec<&str> = rendered.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        result.push_str(line);
        result.push('\n');

        if !injected && line.trim() == "args:" && line.starts_with("      ") {
            let arg_indent = &line[..line.len() - line.trim_start().len()];
            let mut existing: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut j = i + 1;
            while j < lines.len() {
                let next = lines[j].trim();
                if next.starts_with("- ") {
                    let arg_str = next.strip_prefix("- ").unwrap_or(next);
                    let name = arg_str.split(['=', ' ']).next().unwrap_or("").to_string();
                    if !name.is_empty() {
                        existing.insert(name);
                    }
                } else if !next.is_empty() {
                    break;
                }
                j += 1;
            }

            for k in (i + 1)..j {
                result.push_str(lines[k]);
                result.push('\n');
            }

            for arg in args {
                if !existing.contains(arg.as_str()) {
                    result.push_str(arg_indent);
                    result.push_str(&format!("  - {arg}=${{{arg}}}\n"));
                }
            }

            injected = true;
            i = j;
            continue;
        }

        i += 1;
    }

    (result, injected)
}

/// Extract secret names from `RUN --mount=type=secret,id=NAME` in a Dockerfile,
/// returning only those with a matching key in the .env file.
pub fn extract_dockerfile_secrets(dockerfile_path: &Path, env_path: &Path) -> Vec<String> {
    let contents = match std::fs::read_to_string(dockerfile_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let env_keys: std::collections::HashSet<String> = std::fs::read_to_string(env_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            trimmed.split('=').next().map(|k| k.trim().to_string())
        })
        .collect();

    let mut secrets = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in contents.lines() {
        let trimmed = line.trim();
        // Match: --mount=type=secret,id=NAME
        if let Some(pos) = trimmed.find("--mount=type=secret,id=") {
            let after = &trimmed[pos + "--mount=type=secret,id=".len()..];
            let name = after.split([' ', ',', '\\']).next().unwrap_or("").trim();
            if !name.is_empty() && env_keys.contains(name) && seen.insert(name.to_string()) {
                secrets.push(name.to_string());
            }
        }
    }

    secrets
}

/// Inject build secrets config into a rendered compose file.
/// Adds `secrets:` under `build:` and a top-level `secrets:` section.
pub fn inject_build_secrets(rendered: &str, secrets: &[String]) -> String {
    if secrets.is_empty() {
        return rendered.to_string();
    }

    let mut result = String::new();
    let mut injected_build_secrets = false;

    for line in rendered.lines() {
        result.push_str(line);
        result.push('\n');

        // Inject after `dockerfile:` line (same location as args)
        if !injected_build_secrets && line.trim().starts_with("dockerfile:") {
            let indent = &line[..line.len() - line.trim_start().len()];
            result.push_str(indent);
            result.push_str("secrets:\n");
            for secret in secrets {
                result.push_str(indent);
                result.push_str(&format!("  - {secret}\n"));
            }
            injected_build_secrets = true;
        }
    }

    // Append top-level secrets section
    result.push_str("secrets:\n");
    for secret in secrets {
        result.push_str(&format!("  {secret}:\n"));
        result.push_str(&format!("    environment: {secret}\n"));
    }

    result
}

/// Built-in default Rails compose template.
pub fn default_rails_template() -> &'static str {
    r#"services:
  app:
    build:
      context: "{{WORKTREE_PATH}}"
      dockerfile: Dockerfile.devflow
    container_name: devflow-{{WORKER_NAME}}-app
    command: >
      bash -c "rm -f tmp/pids/server.pid && sleep infinity"
    ports:
      - "{{APP_PORT}}:3000"
    volumes:
      - "{{WORKTREE_PATH}}:/app"
    env_file:
      - path: "{{WORKTREE_PATH}}/.env"
        required: false
    environment:
      - RAILS_ENV=development
      - DATABASE_URL=postgres://postgres:postgres@db:5432/{{WORKER_NAME}}_dev
      - REDIS_URL=redis://redis:6379/0
      - BUNDLE_DEPLOYMENT=
      - BUNDLE_WITHOUT=
    depends_on:
      - db
      - redis

  db:
    image: postgres:16-alpine
    container_name: devflow-{{WORKER_NAME}}-db
    ports:
      - "{{DB_PORT}}:5432"
    environment:
      - POSTGRES_USER=postgres
      - POSTGRES_PASSWORD=postgres
      - POSTGRES_DB={{WORKER_NAME}}_dev

  redis:
    image: redis:7-alpine
    container_name: devflow-{{WORKER_NAME}}-redis
    ports:
      - "{{REDIS_PORT}}:6379"
"#
}
