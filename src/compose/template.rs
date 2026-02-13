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

/// Built-in default Rails compose template.
pub fn default_rails_template() -> &'static str {
    r#"services:
  app:
    build:
      context: "{{WORKTREE_PATH}}"
      dockerfile: Dockerfile.devflow
      args:
        - BUNDLE_GITHUB__COM=${BUNDLE_GITHUB__COM:-}
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
