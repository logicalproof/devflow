use std::path::Path;

use crate::error::Result;

use super::ports::AllocatedPorts;

pub struct TemplateVars<'a> {
    pub worker_name: &'a str,
    pub worktree_path: &'a str,
    pub ports: &'a AllocatedPorts,
}

/// Load a user-provided compose template, or fall back to the built-in default.
pub fn load_or_default(devflow_dir: &Path) -> Result<String> {
    let custom_path = devflow_dir.join("compose-template.yml");
    if custom_path.exists() {
        let contents = std::fs::read_to_string(&custom_path)?;
        return Ok(contents);
    }
    Ok(default_rails_template().to_string())
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

/// Extract ARG names from a Dockerfile, returning only those that have a
/// corresponding value in the .env file. ARGs with Dockerfile defaults but
/// no .env entry are left alone so Docker uses the Dockerfile default.
pub fn extract_dockerfile_args(dockerfile_path: &Path, env_path: &Path) -> Vec<String> {
    let contents = match std::fs::read_to_string(dockerfile_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    // Parse .env file to find which vars are defined
    let env_vars: std::collections::HashSet<String> = std::fs::read_to_string(env_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
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
                // Only include if the .env file provides a value for this ARG
                if !name.is_empty() && env_vars.contains(name) {
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

/// Inject build args into a rendered compose file after the `dockerfile:` line.
pub fn inject_build_args(rendered: &str, args: &[String]) -> String {
    if args.is_empty() {
        return rendered.to_string();
    }

    let mut result = String::new();
    let mut injected = false;

    for line in rendered.lines() {
        result.push_str(line);
        result.push('\n');

        // Inject after the `dockerfile:` line, matching its indentation
        if !injected && line.trim().starts_with("dockerfile:") {
            let indent = &line[..line.len() - line.trim_start().len()];
            result.push_str(indent);
            result.push_str("args:\n");
            for arg in args {
                result.push_str(indent);
                result.push_str(&format!("  - {arg}=${{{arg}:-}}\n"));
            }
            injected = true;
        }
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
    ports:
      - "{{APP_PORT}}:3000"
    volumes:
      - "{{WORKTREE_PATH}}:/app"
    env_file:
      - path: "{{WORKTREE_PATH}}/.env"
        required: false
    environment:
      - DATABASE_URL=postgres://postgres:postgres@db:5432/{{WORKER_NAME}}_dev
      - REDIS_URL=redis://redis:6379/0
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
