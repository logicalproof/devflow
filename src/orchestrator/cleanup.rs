use std::path::Path;

use crate::compose::{manager as compose_mgr, ports};
use crate::error::Result;
use crate::tmux::{session, workspace};

use super::state::GroveState;

/// Find orphaned groves (state file exists but tmux session is gone)
pub fn find_orphans(treehouse_dir: &Path) -> Result<Vec<GroveState>> {
    let groves_dir = treehouse_dir.join("groves");
    if !groves_dir.exists() {
        return Ok(Vec::new());
    }

    let mut orphans = Vec::new();
    for entry in std::fs::read_dir(&groves_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            if let Ok(state) = GroveState::load(&path) {
                // Check if the per-grove tmux session still exists
                let session_alive = state
                    .tmux_session
                    .as_ref()
                    .is_some_and(|ws| session::session_exists(ws));
                if !session_alive {
                    orphans.push(state);
                }
            }
        }
    }

    Ok(orphans)
}

/// Clean up an orphaned grove's resources
pub fn cleanup_orphan(treehouse_dir: &Path, repo_root: &Path, state: &GroveState) -> Result<()> {
    // Tear down compose stack if present (best-effort)
    if let Some(ref cf) = state.compose_file {
        let _ = compose_mgr::down(cf);
        let _ = ports::release(treehouse_dir, &state.task_name);
        let compose_dir = treehouse_dir.join("compose").join(&state.task_name);
        let _ = std::fs::remove_dir_all(compose_dir);
    }

    // Tear down per-grove tmux session if present
    if let Some(ref ws) = state.tmux_session {
        workspace::destroy_worker_session(ws);
    }

    // Remove worktree if it exists
    if state.worktree_path.exists() {
        let _ = crate::git::worktree::remove_worktree(repo_root, &state.worktree_path);
    }

    // Remove state file
    let state_path = GroveState::state_path(treehouse_dir, &state.task_name);
    let _ = std::fs::remove_file(state_path);

    // Remove lock file
    let lock_path = treehouse_dir
        .join("locks")
        .join(format!("{}.lock", state.task_name));
    let _ = std::fs::remove_file(lock_path);

    Ok(())
}
