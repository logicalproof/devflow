use std::path::Path;
use std::process::Command;

use crate::error::{DevflowError, Result};

/// Check if tmux is available
pub fn is_available() -> bool {
    which::which("tmux").is_ok()
}

/// Check if a tmux session exists
pub fn session_exists(session_name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", session_name])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Create a new tmux session (detached)
pub fn create_session(session_name: &str, working_dir: &Path) -> Result<()> {
    let output = Command::new("tmux")
        .args(["new-session", "-d", "-s", session_name, "-c"])
        .arg(working_dir)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::TmuxCommand(format!(
            "Failed to create session: {stderr}"
        )));
    }
    Ok(())
}

/// Create a new window in an existing session
pub fn create_window(session_name: &str, window_name: &str, working_dir: &Path) -> Result<()> {
    // Ensure session exists first
    if !session_exists(session_name) {
        create_session(session_name, working_dir)?;
        // Rename the first window (use {end} to handle any base-index setting)
        let output = Command::new("tmux")
            .args([
                "rename-window",
                "-t",
                &format!("{session_name}:{{end}}"),
                window_name,
            ])
            .output()?;
        if !output.status.success() {
            // Fallback: try without token (older tmux)
            let output2 = Command::new("tmux")
                .args([
                    "rename-window",
                    "-t",
                    session_name,
                    window_name,
                ])
                .output()?;
            if !output2.status.success() {
                let stderr = String::from_utf8_lossy(&output2.stderr);
                return Err(DevflowError::TmuxCommand(format!(
                    "Failed to rename window: {stderr}"
                )));
            }
        }
        return Ok(());
    }

    // Use -a to append after the last window, avoiding index collisions
    let output = Command::new("tmux")
        .args(["new-window", "-a", "-t", session_name, "-n", window_name, "-c"])
        .arg(working_dir)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::TmuxCommand(format!(
            "Failed to create window: {stderr}"
        )));
    }
    Ok(())
}

/// Kill a specific window
pub fn kill_window(session_name: &str, window_name: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args([
            "kill-window",
            "-t",
            &format!("{session_name}:{window_name}"),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::TmuxCommand(format!(
            "Failed to kill window: {stderr}"
        )));
    }
    Ok(())
}

/// Check if a window exists in a session
pub fn window_exists(session_name: &str, window_name: &str) -> bool {
    Command::new("tmux")
        .args([
            "list-windows",
            "-t",
            session_name,
            "-F",
            "#{window_name}",
        ])
        .output()
        .is_ok_and(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|line| line == window_name)
        })
}

/// Attach to a session (replaces current terminal)
pub fn attach_session(session_name: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["attach-session", "-t", session_name])
        .status()?;

    if !status.success() {
        return Err(DevflowError::TmuxCommand(
            "Failed to attach to session".to_string(),
        ));
    }
    Ok(())
}

/// Send keys to a specific window in a session
pub fn send_keys(session_name: &str, window_name: &str, command: &str) -> Result<()> {
    let target = format!("{session_name}:{window_name}");
    let output = Command::new("tmux")
        .args(["send-keys", "-t", &target, command, "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::TmuxCommand(format!(
            "Failed to send keys: {stderr}"
        )));
    }
    Ok(())
}

/// Split a window to create a new pane
pub fn split_window(target: &str, working_dir: &Path) -> Result<()> {
    let output = Command::new("tmux")
        .args(["split-window", "-t", target, "-v", "-c"])
        .arg(working_dir)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::TmuxCommand(format!(
            "Failed to split window: {stderr}"
        )));
    }
    Ok(())
}

/// Send keys to a specific pane target (e.g., "session:window.0")
pub fn send_keys_to_pane(target: &str, command: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["send-keys", "-t", target, command, "Enter"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::TmuxCommand(format!(
            "Failed to send keys to pane: {stderr}"
        )));
    }
    Ok(())
}

/// Select (focus) a specific pane
pub fn select_pane(target: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["select-pane", "-t", target])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::TmuxCommand(format!(
            "Failed to select pane: {stderr}"
        )));
    }
    Ok(())
}

/// Apply a layout to a window (e.g., tiled, main-vertical)
pub fn apply_window_layout(target: &str, layout: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["select-layout", "-t", target, layout])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::TmuxCommand(format!(
            "Failed to apply layout: {stderr}"
        )));
    }
    Ok(())
}

/// Kill an entire tmux session
pub fn kill_session(session_name: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(["kill-session", "-t", session_name])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DevflowError::TmuxCommand(format!(
            "Failed to kill session: {stderr}"
        )));
    }
    Ok(())
}

/// List windows in a session
pub fn list_windows(session_name: &str) -> Result<Vec<String>> {
    let output = Command::new("tmux")
        .args([
            "list-windows",
            "-t",
            session_name,
            "-F",
            "#{window_name}",
        ])
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect())
}
