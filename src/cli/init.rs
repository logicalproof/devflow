use std::fs;

use console::style;

use crate::config::local::LocalConfig;
use crate::config::project::ProjectConfig;
use crate::detector;
use crate::error::Result;
use crate::git::repo::GitRepo;

fn ensure_gitignore_entry(repo_root: &std::path::Path, entry: &str) {
    let gitignore_path = repo_root.join(".gitignore");
    let contents = fs::read_to_string(&gitignore_path).unwrap_or_default();

    // Check if any line already matches (ignoring trailing whitespace)
    let already_present = contents
        .lines()
        .any(|line| line.trim() == entry);

    if !already_present {
        let mut new_contents = contents;
        if !new_contents.is_empty() && !new_contents.ends_with('\n') {
            new_contents.push('\n');
        }
        new_contents.push_str(entry);
        new_contents.push('\n');
        let _ = fs::write(&gitignore_path, new_contents);
    }
}

pub async fn run() -> Result<()> {
    let git = GitRepo::discover()?;
    let treehouse_dir = git.treehouse_dir();

    if treehouse_dir.join("config.yml").exists() {
        println!(
            "{} Already initialized. Config at {}",
            style("!").yellow().bold(),
            treehouse_dir.join("config.yml").display()
        );
        return Ok(());
    }

    // Create directory structure
    for dir in &["worktrees", "groves", "locks", "compose"] {
        fs::create_dir_all(treehouse_dir.join(dir))?;
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
    project_config.save(&treehouse_dir.join("config.yml"))?;

    // Write local config
    let local_config = LocalConfig::with_defaults();
    local_config.save(&treehouse_dir.join("local.yml"))?;

    // Ensure .env is in .gitignore to prevent secrets from being committed
    ensure_gitignore_entry(&git.root, ".env");
    ensure_gitignore_entry(&git.root, ".treehouse/worktrees/");
    ensure_gitignore_entry(&git.root, ".treehouse/groves/");
    ensure_gitignore_entry(&git.root, ".treehouse/compose/");
    ensure_gitignore_entry(&git.root, ".treehouse/locks/");
    ensure_gitignore_entry(&git.root, ".treehouse/local.yml");
    ensure_gitignore_entry(&git.root, ".treehouse/ports.json");
    ensure_gitignore_entry(&git.root, ".treehouse/ports.json.lock");
    println!(
        "{} Initialized treehouse for project '{}'",
        style("✓").green().bold(),
        project_name
    );
    println!(
        "  Config: {}",
        treehouse_dir.join("config.yml").display()
    );

    Ok(())
}
