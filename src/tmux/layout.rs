use std::process::Command;

use crate::error::{GrootError, Result};

pub const VALID_LAYOUTS: &[&str] = &[
    "tiled",
    "even-horizontal",
    "even-vertical",
    "main-horizontal",
    "main-vertical",
];

pub fn apply_layout(session_name: &str, layout: &str) -> Result<()> {
    if !VALID_LAYOUTS.contains(&layout) {
        return Err(GrootError::Other(format!(
            "Invalid layout: {layout}. Valid layouts: {}",
            VALID_LAYOUTS.join(", ")
        )));
    }

    let output = Command::new("tmux")
        .args(["select-layout", "-t", session_name, layout])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GrootError::TmuxCommand(format!(
            "Failed to apply layout: {stderr}"
        )));
    }
    Ok(())
}
