use std::path::Path;
use std::time::Duration;

use sysinfo::Disks;

use crate::compose::{manager as compose_mgr, ports};
use crate::config::lock::FileLock;
use crate::error::{DevflowError, Result};
use crate::git::{branch, repo::GitRepo, worktree};
use crate::tmux::session;

use super::state::WorkerState;

/// Spawn a new worker: create branch, worktree, optionally start compose stack,
/// create tmux window, save state.
pub fn spawn(
    git: &GitRepo,
    devflow_dir: &Path,
    task_name: &str,
    branch_name: &str,
    tmux_session: &str,
    min_disk_mb: u64,
    initial_command: Option<&str>,
    enable_compose: bool,
    compose_health_timeout_secs: u64,
    compose_post_start: &[String],
) -> Result<WorkerState> {
    // 1. Acquire lock
    let lock_path = devflow_dir.join("locks").join(format!("{task_name}.lock"));
    let _lock = FileLock::acquire(&lock_path)?;

    // 2. Check for duplicate worker
    let state_path = WorkerState::state_path(devflow_dir, task_name);
    if state_path.exists() {
        return Err(DevflowError::WorkerAlreadyExists(task_name.to_string()));
    }

    // 3. Check disk space
    check_disk_space(min_disk_mb)?;

    // 4. Create branch (skip if it already exists from task creation)
    let branch_created = if !branch::branch_exists(git, branch_name) {
        branch::create_branch(git, branch_name)?;
        true
    } else {
        false
    };

    // 5. Create worktree
    let worktree_path = devflow_dir.join("worktrees").join(task_name);
    if let Err(e) = worktree::create_worktree(&git.root, &worktree_path, branch_name) {
        if branch_created {
            let _ = branch::delete_branch(git, branch_name);
        }
        return Err(e);
    }

    // 5a-5d. Optionally start compose stack
    let mut compose_file = None;
    let mut compose_ports = None;

    if enable_compose {
        // 5a. Check docker compose is available
        if let Err(e) = compose_mgr::check_available() {
            let _ = worktree::remove_worktree(&git.root, &worktree_path);
            if branch_created {
                let _ = branch::delete_branch(git, branch_name);
            }
            return Err(e);
        }

        // 5b. Allocate ports
        let allocated = match ports::allocate(devflow_dir, task_name) {
            Ok(p) => p,
            Err(e) => {
                let _ = worktree::remove_worktree(&git.root, &worktree_path);
                if branch_created {
                    let _ = branch::delete_branch(git, branch_name);
                }
                return Err(e);
            }
        };

        // 5b½. Check ports are actually available on the host
        if let Err(e) = ports::check_ports_available(&allocated) {
            let _ = ports::release(devflow_dir, task_name);
            let _ = worktree::remove_worktree(&git.root, &worktree_path);
            if branch_created {
                let _ = branch::delete_branch(git, branch_name);
            }
            return Err(e);
        }

        // 5c. Generate compose file
        let cf = match compose_mgr::generate_compose_file(
            devflow_dir,
            task_name,
            &worktree_path,
            &allocated,
        ) {
            Ok(cf) => cf,
            Err(e) => {
                let _ = ports::release(devflow_dir, task_name);
                let _ = worktree::remove_worktree(&git.root, &worktree_path);
                if branch_created {
                    let _ = branch::delete_branch(git, branch_name);
                }
                return Err(e);
            }
        };

        // 5d. Start compose stack
        if let Err(e) = compose_mgr::up(&cf) {
            let _ = ports::release(devflow_dir, task_name);
            let compose_dir = devflow_dir.join("compose").join(task_name);
            let _ = std::fs::remove_dir_all(compose_dir);
            let _ = worktree::remove_worktree(&git.root, &worktree_path);
            if branch_created {
                let _ = branch::delete_branch(git, branch_name);
            }
            return Err(e);
        }

        // 5e. Wait for containers to be healthy
        if let Err(e) = compose_mgr::wait_healthy(
            &cf,
            Duration::from_secs(compose_health_timeout_secs),
        ) {
            let _ = compose_mgr::down(&cf);
            let _ = ports::release(devflow_dir, task_name);
            let compose_dir = devflow_dir.join("compose").join(task_name);
            let _ = std::fs::remove_dir_all(compose_dir);
            let _ = worktree::remove_worktree(&git.root, &worktree_path);
            if branch_created {
                let _ = branch::delete_branch(git, branch_name);
            }
            return Err(e);
        }

        // 5f. Run post-start hooks (warn on failure, don't tear down)
        for hook in compose_post_start {
            println!("Running post-start hook: {hook}");
            match compose_mgr::exec(&cf, "app", hook) {
                Ok(()) => println!("  Hook succeeded: {hook}"),
                Err(e) => eprintln!("  Warning: hook failed: {e}"),
            }
        }

        compose_file = Some(cf);
        compose_ports = Some(allocated);
    }

    // 6. Create tmux window
    let tmux_window = task_name.to_string();
    if let Err(e) = session::create_window(tmux_session, &tmux_window, &worktree_path) {
        // Rollback compose if it was started
        if let Some(ref cf) = compose_file {
            let _ = compose_mgr::down(cf);
            let _ = ports::release(devflow_dir, task_name);
            let compose_dir = devflow_dir.join("compose").join(task_name);
            let _ = std::fs::remove_dir_all(compose_dir);
        }
        let _ = worktree::remove_worktree(&git.root, &worktree_path);
        if branch_created {
            let _ = branch::delete_branch(git, branch_name);
        }
        return Err(e);
    }

    // 7. Send initial command if provided
    if let Some(cmd) = initial_command {
        if let Err(e) = session::send_keys(tmux_session, &tmux_window, cmd) {
            eprintln!("Warning: failed to send initial command: {e}");
        }
    }

    // 8. Save worker state
    let state = WorkerState {
        task_name: task_name.to_string(),
        branch: branch_name.to_string(),
        worktree_path: worktree_path.clone(),
        tmux_window: tmux_window.clone(),
        container_id: None,
        created_at: chrono::Utc::now(),
        pid: None,
        compose_file,
        compose_ports,
    };

    if let Err(e) = state.save(&state_path) {
        // Rollback compose if it was started
        if let Some(ref cf) = state.compose_file {
            let _ = compose_mgr::down(cf);
            let _ = ports::release(devflow_dir, task_name);
            let compose_dir = devflow_dir.join("compose").join(task_name);
            let _ = std::fs::remove_dir_all(compose_dir);
        }
        let _ = session::kill_window(tmux_session, &tmux_window);
        let _ = worktree::remove_worktree(&git.root, &worktree_path);
        if branch_created {
            let _ = branch::delete_branch(git, branch_name);
        }
        return Err(e);
    }

    Ok(state)
}

/// Kill a worker: tear down compose stack, remove tmux window, worktree, branch, and state file
pub fn kill(git: &GitRepo, devflow_dir: &Path, task_name: &str, tmux_session: &str) -> Result<()> {
    let state_path = WorkerState::state_path(devflow_dir, task_name);
    if !state_path.exists() {
        return Err(DevflowError::WorkerNotFound(task_name.to_string()));
    }

    let state = WorkerState::load(&state_path)?;

    // Tear down compose stack if present
    if let Some(ref cf) = state.compose_file {
        if let Err(e) = compose_mgr::down(cf) {
            eprintln!("Warning: compose down failed: {e}");
        }
        let _ = ports::release(devflow_dir, task_name);
        let compose_dir = devflow_dir.join("compose").join(task_name);
        let _ = std::fs::remove_dir_all(compose_dir);
    }

    // Kill tmux window (ignore errors - may already be gone)
    let _ = session::kill_window(tmux_session, &state.tmux_window);

    // Remove worktree
    if state.worktree_path.exists() {
        worktree::remove_worktree(&git.root, &state.worktree_path)?;
    }

    // Delete branch
    let _ = branch::delete_branch(git, &state.branch);

    // Remove state file
    std::fs::remove_file(&state_path)?;

    // Remove lock file if it exists
    let lock_path = devflow_dir.join("locks").join(format!("{task_name}.lock"));
    let _ = std::fs::remove_file(lock_path);

    Ok(())
}

/// List all workers from state files
pub fn list_workers(devflow_dir: &Path) -> Result<Vec<WorkerState>> {
    let workers_dir = devflow_dir.join("workers");
    if !workers_dir.exists() {
        return Ok(Vec::new());
    }

    let mut workers = Vec::new();
    for entry in std::fs::read_dir(&workers_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            if let Ok(state) = WorkerState::load(&path) {
                workers.push(state);
            }
        }
    }

    Ok(workers)
}

fn check_disk_space(min_mb: u64) -> Result<()> {
    let disks = Disks::new_with_refreshed_list();
    for disk in disks.list() {
        if disk.mount_point() == Path::new("/") {
            let available_mb = disk.available_space() / (1024 * 1024);
            if available_mb < min_mb {
                return Err(DevflowError::InsufficientDiskSpace {
                    available_mb,
                    required_mb: min_mb,
                });
            }
            return Ok(());
        }
    }
    // If we can't determine disk space, proceed anyway
    Ok(())
}
