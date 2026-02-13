use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

use super::session;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkspaceTemplate {
    pub windows: Vec<WindowTemplate>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WindowTemplate {
    pub name: String,
    #[serde(default = "default_layout")]
    pub layout: String,
    #[serde(default)]
    pub panes: Vec<PaneTemplate>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaneTemplate {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub focus: bool,
}

fn default_layout() -> String {
    "tiled".to_string()
}

pub struct WorkspaceVars<'a> {
    pub worktree_path: &'a str,
    pub worker_name: &'a str,
    pub app_port: Option<u16>,
    pub db_port: Option<u16>,
    pub redis_port: Option<u16>,
}

/// Load a workspace template from `.devflow/tmux-layout.json`.
/// Returns `None` if the file doesn't exist.
pub fn load_template(devflow_dir: &Path) -> Result<Option<WorkspaceTemplate>> {
    let path = devflow_dir.join("tmux-layout.json");
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)?;
    let template: WorkspaceTemplate = serde_json::from_str(&contents)?;
    Ok(Some(template))
}

/// Replace `{{VAR}}` placeholders in all command and directory strings.
pub fn render_template(template: &WorkspaceTemplate, vars: &WorkspaceVars) -> WorkspaceTemplate {
    let substitute = |s: &str| -> String {
        let mut result = s.to_string();
        result = result.replace("{{WORKTREE_PATH}}", vars.worktree_path);
        result = result.replace("{{WORKER_NAME}}", vars.worker_name);
        result = result.replace("{{APP_PORT}}", &vars.app_port.unwrap_or(3000).to_string());
        result = result.replace("{{DB_PORT}}", &vars.db_port.unwrap_or(5432).to_string());
        result = result.replace("{{REDIS_PORT}}", &vars.redis_port.unwrap_or(6379).to_string());
        result
    };

    WorkspaceTemplate {
        windows: template
            .windows
            .iter()
            .map(|w| WindowTemplate {
                name: substitute(&w.name),
                layout: w.layout.clone(),
                panes: w
                    .panes
                    .iter()
                    .map(|p| PaneTemplate {
                        command: p.command.as_ref().map(|c| substitute(c)),
                        directory: p.directory.as_ref().map(|d| substitute(d)),
                        focus: p.focus,
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Compute the per-worker session name: `"{hub}-{task}"`.
pub fn worker_session_name(hub_session: &str, task_name: &str) -> String {
    format!("{hub_session}-{task_name}")
}

/// Check if a per-worker session exists.
pub fn worker_session_exists(hub_session: &str, task_name: &str) -> bool {
    let name = worker_session_name(hub_session, task_name);
    session::session_exists(&name)
}

/// Create a per-worker tmux session with windows and panes from the template.
pub fn create_worker_session(
    session_name: &str,
    template: &WorkspaceTemplate,
    default_dir: &Path,
) -> Result<()> {
    for (win_idx, window) in template.windows.iter().enumerate() {
        let win_target = format!("{session_name}:{}", window.name);

        if win_idx == 0 {
            // Create the session with the first window
            session::create_session(session_name, default_dir)?;
            // Rename the default window (target the session itself, not a hardcoded index)
            let rename_output = std::process::Command::new("tmux")
                .args([
                    "rename-window",
                    "-t",
                    session_name,
                    &window.name,
                ])
                .output()?;
            if !rename_output.status.success() {
                let stderr = String::from_utf8_lossy(&rename_output.stderr);
                return Err(crate::error::DevflowError::TmuxCommand(format!(
                    "Failed to rename window: {stderr}"
                )));
            }
        } else {
            // Create additional windows
            session::create_window(session_name, &window.name, default_dir)?;
        }

        // Create panes (first pane already exists as pane 0)
        let mut focus_pane: Option<usize> = None;
        for (pane_idx, pane) in window.panes.iter().enumerate() {
            let pane_dir = pane
                .directory
                .as_ref()
                .map(|d| std::path::PathBuf::from(d))
                .unwrap_or_else(|| default_dir.to_path_buf());

            if pane_idx > 0 {
                // Split to create additional panes
                session::split_window(&win_target, &pane_dir)?;
            } else if pane.directory.is_some() {
                // For the first pane, send a cd if a custom directory is set
                let pane_target = format!("{win_target}.0");
                session::send_keys_to_pane(&pane_target, &format!("cd {}", pane_dir.display()))?;
            }

            if pane.focus {
                focus_pane = Some(pane_idx);
            }
        }

        // Apply layout after all panes are created
        if window.panes.len() > 1 {
            session::apply_window_layout(&win_target, &window.layout)?;
        }

        // Send commands to each pane
        for (pane_idx, pane) in window.panes.iter().enumerate() {
            if let Some(ref cmd) = pane.command {
                let pane_target = format!("{win_target}.{pane_idx}");
                session::send_keys_to_pane(&pane_target, cmd)?;
            }
        }

        // Select the focused pane
        if let Some(idx) = focus_pane {
            let pane_target = format!("{win_target}.{idx}");
            session::select_pane(&pane_target)?;
        }
    }

    Ok(())
}

/// Destroy a per-worker session. Ignores errors (session may already be gone).
pub fn destroy_worker_session(session_name: &str) {
    let _ = session::kill_session(session_name);
}

/// Return a built-in Rails development workspace template.
pub fn default_template() -> WorkspaceTemplate {
    WorkspaceTemplate {
        windows: vec![
            WindowTemplate {
                name: "server".to_string(),
                layout: "tiled".to_string(),
                panes: vec![
                    PaneTemplate {
                        command: Some("tail -f log/development.log".to_string()),
                        directory: None,
                        focus: false,
                    },
                    PaneTemplate {
                        command: Some("bundle exec puma -p {{APP_PORT}}".to_string()),
                        directory: None,
                        focus: false,
                    },
                    PaneTemplate {
                        command: Some("bundle exec sidekiq".to_string()),
                        directory: None,
                        focus: false,
                    },
                    PaneTemplate {
                        command: None,
                        directory: None,
                        focus: false,
                    },
                ],
            },
            WindowTemplate {
                name: "editor".to_string(),
                layout: "main-vertical".to_string(),
                panes: vec![
                    PaneTemplate {
                        command: Some("vim".to_string()),
                        directory: None,
                        focus: true,
                    },
                    PaneTemplate {
                        command: None,
                        directory: None,
                        focus: false,
                    },
                    PaneTemplate {
                        command: Some("claude".to_string()),
                        directory: None,
                        focus: false,
                    },
                    PaneTemplate {
                        command: Some("rails console".to_string()),
                        directory: None,
                        focus: false,
                    },
                ],
            },
        ],
    }
}
