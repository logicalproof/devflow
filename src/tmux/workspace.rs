use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::compose::manager as compose_mgr;
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
    /// When `true`, this pane always runs on the host even when compose is active.
    /// When `false` (default), pane commands are wrapped with `docker compose exec`
    /// if a compose stack is running.
    #[serde(default)]
    pub host: bool,
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
    pub compose_file: Option<&'a Path>,
}

/// Load a workspace template from `.groot/tmux-layout.json`.
/// Returns `None` if the file doesn't exist.
pub fn load_template(groot_dir: &Path) -> Result<Option<WorkspaceTemplate>> {
    let path = groot_dir.join("tmux-layout.json");
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)?;
    let template: WorkspaceTemplate = serde_json::from_str(&contents)?;
    Ok(Some(template))
}

/// Replace `{{VAR}}` placeholders in all command and directory strings.
pub fn render_template(template: &WorkspaceTemplate, vars: &WorkspaceVars) -> WorkspaceTemplate {
    let compose_file_str = vars
        .compose_file
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let compose_project_str = vars
        .compose_file
        .map(|p| compose_mgr::project_name(p))
        .unwrap_or_default();

    let substitute = |s: &str| -> String {
        let mut result = s.to_string();
        result = result.replace("{{WORKTREE_PATH}}", vars.worktree_path);
        result = result.replace("{{WORKER_NAME}}", vars.worker_name);
        result = result.replace("{{APP_PORT}}", &vars.app_port.unwrap_or(3000).to_string());
        result = result.replace("{{DB_PORT}}", &vars.db_port.unwrap_or(5432).to_string());
        result = result.replace("{{REDIS_PORT}}", &vars.redis_port.unwrap_or(6379).to_string());
        result = result.replace("{{COMPOSE_FILE}}", &compose_file_str);
        result = result.replace("{{COMPOSE_PROJECT}}", &compose_project_str);
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
                        host: p.host,
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

/// Query tmux's base-index setting (default 0, some users set to 1).
fn get_base_index() -> u32 {
    std::process::Command::new("tmux")
        .args(["show-option", "-gv", "base-index"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8_lossy(&o.stdout).trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

/// Create a per-worker tmux session with windows and panes from the template.
///
/// When `compose_file` is `Some`, non-host panes have their commands wrapped with
/// `docker compose exec app` so they run inside the container instead of on the host.
pub fn create_worker_session(
    session_name: &str,
    template: &WorkspaceTemplate,
    default_dir: &Path,
    compose_file: Option<&Path>,
) -> Result<()> {
    // Build the exec prefix once if compose is active
    let exec_prefix = compose_file.map(|cf| {
        let project = compose_mgr::project_name(cf);
        format!(
            "docker compose -f \"{}\" -p \"{}\" exec app",
            cf.to_string_lossy(),
            project,
        )
    });

    // Use window indices for targeting — window names are unreliable due to
    // automatic-rename and other tmux config that can change names after creation.
    let base_index = get_base_index();

    for (win_idx, window) in template.windows.iter().enumerate() {
        let win_index = base_index + win_idx as u32;
        let win_target = format!("{session_name}:{win_index}");

        if win_idx == 0 {
            // Create the session with the first window
            session::create_session(session_name, default_dir)?;
            // Rename the default window (best-effort, purely cosmetic)
            let _ = std::process::Command::new("tmux")
                .args(["rename-window", "-t", &win_target, &window.name])
                .output();
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
            let pane_target = format!("{win_target}.{pane_idx}");

            match (&exec_prefix, pane.host) {
                // Compose active + non-host pane: wrap command with docker compose exec
                (Some(prefix), false) => {
                    if let Some(ref cmd) = pane.command {
                        session::send_keys_to_pane(&pane_target, &format!("{prefix} {cmd}"))?;
                    } else {
                        session::send_keys_to_pane(&pane_target, &format!("{prefix} bash"))?;
                    }
                }
                // Host pane or no compose: run command directly (current behavior)
                _ => {
                    if let Some(ref cmd) = pane.command {
                        session::send_keys_to_pane(&pane_target, cmd)?;
                    }
                }
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
                        host: false,
                    },
                    PaneTemplate {
                        command: Some("rails console".to_string()),
                        directory: None,
                        focus: false,
                        host: false,
                    },
                    PaneTemplate {
                        command: Some("bundle exec sidekiq".to_string()),
                        directory: None,
                        focus: false,
                        host: false,
                    },
                    PaneTemplate {
                        command: None,
                        directory: None,
                        focus: false,
                        host: false,
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
                        host: true,
                    },
                    PaneTemplate {
                        command: None,
                        directory: None,
                        focus: false,
                        host: true,
                    },
                    PaneTemplate {
                        command: Some("claude".to_string()),
                        directory: None,
                        focus: false,
                        host: true,
                    },
                    PaneTemplate {
                        command: Some("rails console".to_string()),
                        directory: None,
                        focus: false,
                        host: false,
                    },
                ],
            },
        ],
    }
}
