use std::path::Path;

use console::style;
use dialoguer::{Confirm, Select};

use crate::config::project::ProjectConfig;
use crate::container::templates;
use crate::error::{TreehouseError, Result};
use crate::git::repo::GitRepo;

fn select_and_write_template(repo_root: &Path, dockerfile_path: &Path) -> Result<String> {
    let options = vec!["Rails", "React Native", "Custom (Ubuntu base)"];

    let selection = Select::new()
        .with_prompt("Select a container template")
        .items(&options)
        .default(0)
        .interact()
        .map_err(|e| TreehouseError::Other(format!("Selection cancelled: {e}")))?;

    let dockerfile_content = match selection {
        0 => templates::rails_template().to_string(),
        1 => templates::react_native_template().to_string(),
        _ => {
            "FROM ubuntu:22.04\nRUN apt-get update -qq && apt-get install -y git curl\nWORKDIR /app\nCMD [\"sleep\", \"infinity\"]\n".to_string()
        }
    };

    let template_name = options[selection];
    println!();
    println!("Template: {}", style(template_name).cyan());
    println!();
    println!("{}", &dockerfile_content);

    let proceed = Confirm::new()
        .with_prompt("Write Dockerfile to project?")
        .default(true)
        .interact()
        .map_err(|e| TreehouseError::Other(format!("Confirm cancelled: {e}")))?;

    if !proceed {
        return Err(TreehouseError::Other("Cancelled.".to_string()));
    }

    std::fs::write(dockerfile_path, &dockerfile_content)?;

    println!(
        "{} Wrote {}",
        style("✓").green().bold(),
        dockerfile_path.display()
    );
    println!(
        "Build with: {}",
        style("th grove build <name>").cyan()
    );

    let _ = repo_root; // available for future use
    Ok(dockerfile_content)
}

pub async fn run() -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = git.treehouse_dir();

    if !treehouse_dir.join("config.yml").exists() {
        return Err(TreehouseError::NotInitialized);
    }

    let config = ProjectConfig::load(&treehouse_dir.join("config.yml"))?;

    println!("{}", style("Container Setup Wizard").bold());
    println!();

    // Check for existing Dockerfiles
    let existing_dockerfiles: Vec<(&str, std::path::PathBuf)> = [
        "Dockerfile",
        "Dockerfile.devflow",
        "Dockerfile.dev",
        "dockerfile",
    ]
    .iter()
    .filter_map(|name| {
        let path = git.root.join(name);
        if path.exists() {
            Some((*name, path))
        } else {
            None
        }
    })
    .collect();

    let dockerfile_content;
    let dockerfile_path = git.root.join("Dockerfile.devflow");

    if !existing_dockerfiles.is_empty() {
        println!(
            "{} Found existing Dockerfile(s):",
            style("!").yellow()
        );
        for (name, path) in &existing_dockerfiles {
            println!("  - {}", path.display());
            let _ = name; // used for display via path
        }
        println!();

        let mut use_options: Vec<String> = existing_dockerfiles
            .iter()
            .map(|(name, _)| format!("Use existing {name}"))
            .collect();
        use_options.push("Generate new from template".to_string());

        let selection = Select::new()
            .with_prompt("Which Dockerfile should compose use?")
            .items(&use_options)
            .default(0)
            .interact()
            .map_err(|e| TreehouseError::Other(format!("Selection cancelled: {e}")))?;

        if selection < existing_dockerfiles.len() {
            // Copy existing Dockerfile to Dockerfile.devflow if it isn't already
            let (name, source_path) = &existing_dockerfiles[selection];
            if *name != "Dockerfile.devflow" {
                let content = std::fs::read_to_string(source_path)?;
                std::fs::write(&dockerfile_path, &content)?;
                println!(
                    "{} Copied {} to {}",
                    style("✓").green().bold(),
                    name,
                    dockerfile_path.display()
                );
            } else {
                println!(
                    "{} Using existing {}",
                    style("✓").green().bold(),
                    dockerfile_path.display()
                );
            }
            dockerfile_content = std::fs::read_to_string(&dockerfile_path)?;
        } else {
            // Fall through to template selection
            dockerfile_content = select_and_write_template(&git.root, &dockerfile_path)?;
        }
    } else {
        dockerfile_content = select_and_write_template(&git.root, &dockerfile_path)?;
    }

    let _ = &dockerfile_content;

    // Update config
    let mut config = config;
    config.container_enabled = true;
    config.save(&treehouse_dir.join("config.yml"))?;

    // Offer to generate compose template for per-worker stacks
    let generate_compose = Confirm::new()
        .with_prompt("Generate Docker Compose template for per-worker stacks?")
        .default(true)
        .interact()
        .map_err(|e| TreehouseError::Other(format!("Confirm cancelled: {e}")))?;

    if generate_compose {
        let template_content = crate::compose::template::default_rails_template();
        let template_path = treehouse_dir.join("compose-template.yml");
        std::fs::write(&template_path, template_content)?;

        println!(
            "{} Wrote {}",
            style("✓").green().bold(),
            template_path.display()
        );
        println!(
            "Use with: {}",
            style("th grove plant <task>").cyan()
        );
    }

    Ok(())
}
