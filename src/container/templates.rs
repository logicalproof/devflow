use std::path::Path;

use crate::error::{TreehouseError, Result};

pub struct DockerfileTemplate {
    pub name: String,
    pub content: String,
}

/// Load a Dockerfile template from the templates directory
pub fn load_template(templates_dir: &Path, name: &str) -> Result<DockerfileTemplate> {
    let path = templates_dir.join(format!("Dockerfile.{name}"));
    if !path.exists() {
        return Err(TreehouseError::TemplateNotFound(name.to_string()));
    }

    let content = std::fs::read_to_string(&path)?;
    Ok(DockerfileTemplate {
        name: name.to_string(),
        content,
    })
}

/// List available templates
pub fn list_templates(templates_dir: &Path) -> Vec<String> {
    let mut templates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(templates_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(template_name) = name.strip_prefix("Dockerfile.") {
                templates.push(template_name.to_string());
            }
        }
    }
    templates
}

/// Built-in Rails Dockerfile template
pub fn rails_template() -> &'static str {
    r#"FROM ruby:3.3-slim

RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends \
    build-essential libpq-dev nodejs npm git curl && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

CMD ["sleep", "infinity"]
"#
}

/// Built-in React Native Dockerfile template
pub fn react_native_template() -> &'static str {
    r#"FROM node:20-slim

RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends \
    git curl watchman && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

CMD ["sleep", "infinity"]
"#
}
