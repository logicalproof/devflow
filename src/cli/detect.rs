use console::style;

use crate::detector;
use crate::error::Result;
use crate::git::repo::GitRepo;

pub async fn run() -> Result<()> {
    let git = GitRepo::discover()?;
    let detected = detector::detect_project_types(&git.root);

    if detected.is_empty() {
        println!("{} No project types detected", style("!").yellow());
    } else {
        println!("{} Detected project types:", style("✓").green().bold());
        for t in &detected {
            println!("  - {t}");
        }
    }

    Ok(())
}
