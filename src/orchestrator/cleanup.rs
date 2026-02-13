use std::path::Path;

use crate::compose::{manager as compose_mgr, ports};
use crate::error::Result;
use crate::tmux::{session, workspace};

use super::state::WorkerState;

/// Find orphaned workers (state file exists but tmux window is gone)
pub fn find_orphans(devflow_dir: &Path, tmux_session: &str) -> Result<Vec<WorkerState>> {
    let workers_dir = devflow_dir.join("workers");
    if !workers_dir.exists() {
        return Ok(Vec::new());
    }

    let mut orphans = Vec::new();
    for entry in std::fs::read_dir(&workers_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            if let Ok(state) = WorkerState::load(&path) {
                // Check if the tmux window still exists
                if !session::window_exists(tmux_session, &state.tmux_window) {
                    orphans.push(state);
                }
            }
        }
    }

    Ok(orphans)
}

/// Clean up an orphaned worker's resources
pub fn cleanup_orphan(devflow_dir: &Path, repo_root: &Path, state: &WorkerState) -> Result<()> {
    // Tear down compose stack if present (best-effort)
    if let Some(ref cf) = state.compose_file {
        let _ = compose_mgr::down(cf);
        let _ = ports::release(devflow_dir, &state.task_name);
        let compose_dir = devflow_dir.join("compose").join(&state.task_name);
        let _ = std::fs::remove_dir_all(compose_dir);
    }

    // Tear down per-worker tmux session if present
    if let Some(ref ws) = state.tmux_session {
        workspace::destroy_worker_session(ws);
    }

    // Remove worktree if it exists
    if state.worktree_path.exists() {
        let _ = crate::git::worktree::remove_worktree(repo_root, &state.worktree_path);
    }

    // Remove state file
    let state_path = WorkerState::state_path(devflow_dir, &state.task_name);
    let _ = std::fs::remove_file(state_path);

    // Remove lock file
    let lock_path = devflow_dir
        .join("locks")
        .join(format!("{}.lock", state.task_name));
    let _ = std::fs::remove_file(lock_path);

    Ok(())
}
