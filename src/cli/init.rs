use std::fs;

use console::style;

use crate::config::local::LocalConfig;
use crate::config::project::ProjectConfig;
use crate::detector;
use crate::error::Result;
use crate::git::repo::GitRepo;

pub async fn run() -> Result<()> {
    let git = GitRepo::discover()?;
    let devflow_dir = git.devflow_dir();

    if devflow_dir.join("config.yml").exists() {
        println!(
            "{} Already initialized. Config at {}",
            style("!").yellow().bold(),
            devflow_dir.join("config.yml").display()
        );
        return Ok(());
    }

    // Create directory structure
    for dir in &["worktrees", "workers", "locks", "tasks"] {
        fs::create_dir_all(devflow_dir.join(dir))?;
    }

    // Detect project types
    let detected = detector::detect_project_types(&git.root);
    if detected.is_empty() {
        println!("{} No project types detected", style("!").yellow());
    } else {
        println!(
            "{} Detected: {}",
            style("✓").green().bold(),
            detected.join(", ")
        );
    }

    // Infer project name from directory
    let project_name = git
        .root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());

    // Get default branch
    let default_branch = git
        .repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_else(|| "main".to_string());

    // Write project config
    let project_config = ProjectConfig {
        project_name: project_name.clone(),
        detected_types: detected,
        container_enabled: false,
        default_branch,
    };
    project_config.save(&devflow_dir.join("config.yml"))?;

    // Write local config
    let local_config = LocalConfig::with_defaults();
    local_config.save(&devflow_dir.join("local.yml"))?;

    // Create empty tasks file
    fs::write(devflow_dir.join("tasks.json"), "[]")?;

    println!(
        "{} Initialized devflow for project '{}'",
        style("✓").green().bold(),
        project_name
    );
    println!(
        "  Config: {}",
        devflow_dir.join("config.yml").display()
    );

    Ok(())
}
