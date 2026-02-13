use console::style;
use dialoguer::{Confirm, Select};

use crate::config::project::ProjectConfig;
use crate::container::templates;
use crate::error::{DevflowError, Result};
use crate::git::repo::GitRepo;

pub async fn run() -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();

    if !devflow_dir.join("config.yml").exists() {
        return Err(DevflowError::NotInitialized);
    }

    let config = ProjectConfig::load(&devflow_dir.join("config.yml"))?;

    println!("{}", style("Container Setup Wizard").bold());
    println!();

    // Determine available templates
    let options = vec!["Rails", "React Native", "Custom (Ubuntu base)"];

    let selection = Select::new()
        .with_prompt("Select a container template")
        .items(&options)
        .default(0)
        .interact()
        .map_err(|e| DevflowError::Other(format!("Selection cancelled: {e}")))?;

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
        .map_err(|e| DevflowError::Other(format!("Confirm cancelled: {e}")))?;

    if !proceed {
        println!("Cancelled.");
        return Ok(());
    }

    // Write Dockerfile
    let dockerfile_path = git.root.join("Dockerfile.devflow");
    std::fs::write(&dockerfile_path, &dockerfile_content)?;

    // Update config
    let mut config = config;
    config.container_enabled = true;
    config.save(&devflow_dir.join("config.yml"))?;

    println!(
        "{} Wrote {}",
        style("✓").green().bold(),
        dockerfile_path.display()
    );
    println!(
        "Build with: {}",
        style("devflow container build <name>").cyan()
    );

    Ok(())
}
