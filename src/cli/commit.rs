use console::style;
use dialoguer::{Input, Select};

use crate::error::{DevflowError, Result};

const COMMIT_TYPES: &[(&str, &str)] = &[
    ("feat", "A new feature"),
    ("fix", "A bug fix"),
    ("docs", "Documentation only changes"),
    ("style", "Code style changes (formatting, etc)"),
    ("refactor", "Code refactoring"),
    ("perf", "Performance improvement"),
    ("test", "Adding or updating tests"),
    ("chore", "Build process or auxiliary tool changes"),
    ("ci", "CI configuration changes"),
];

pub async fn run() -> Result<()> {
    println!("{}", style("Conventional Commit Helper").bold());
    println!();

    // Check for staged changes
    let status_output = std::process::Command::new("git")
        .args(["diff", "--cached", "--stat"])
        .output()?;

    let staged = String::from_utf8_lossy(&status_output.stdout);
    if staged.trim().is_empty() {
        println!("No staged changes. Stage files first with: git add <files>");
        return Ok(());
    }

    println!("{}", style("Staged changes:").bold());
    println!("{staged}");

    // Select commit type
    let type_labels: Vec<String> = COMMIT_TYPES
        .iter()
        .map(|(t, desc)| format!("{t}: {desc}"))
        .collect();

    let type_idx = Select::new()
        .with_prompt("Commit type")
        .items(&type_labels)
        .default(0)
        .interact()
        .map_err(|e| DevflowError::Other(format!("Selection cancelled: {e}")))?;

    let commit_type = COMMIT_TYPES[type_idx].0;

    // Optional scope
    let scope: String = Input::new()
        .with_prompt("Scope (optional, press Enter to skip)")
        .allow_empty(true)
        .interact_text()
        .map_err(|e| DevflowError::Other(format!("Input cancelled: {e}")))?;

    // Commit message
    let message: String = Input::new()
        .with_prompt("Short description")
        .interact_text()
        .map_err(|e| DevflowError::Other(format!("Input cancelled: {e}")))?;

    // Build the commit message
    let full_message = if scope.is_empty() {
        format!("{commit_type}: {message}")
    } else {
        format!("{commit_type}({scope}): {message}")
    };

    println!();
    println!("Commit message: {}", style(&full_message).green());

    // Execute commit
    let output = std::process::Command::new("git")
        .args(["commit", "-m", &full_message])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::GitCommand(format!(
            "Commit failed: {stderr}"
        )));
    }

    println!("{} Committed!", style("✓").green().bold());

    Ok(())
}
