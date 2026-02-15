use std::path::Path;
use std::time::Duration;

use sysinfo::Disks;

use crate::claude_md;
use crate::cli::task as cli_task;
use crate::compose::{db as compose_db, manager as compose_mgr, ports};
use crate::config::lock::FileLock;
use crate::config::project::ProjectConfig;
use crate::error::{DevflowError, Result};
use crate::git::{branch, repo::GitRepo, worktree};
use crate::tmux::{session, workspace};

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
    db_clone: bool,
    db_source: Option<&str>,
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

    // 5. Create worktree (or reuse existing one from a previous `stop`)
    let worktree_path = devflow_dir.join("worktrees").join(task_name);
    let reusing_worktree = worktree::worktree_exists(&worktree_path);

    if reusing_worktree {
        println!("Reusing existing worktree at {}", worktree_path.display());
    } else if let Err(e) = worktree::create_worktree(&git.root, &worktree_path, branch_name) {
        if branch_created {
            let _ = branch::delete_branch(git, branch_name);
        }
        return Err(e);
    }

    // 5½. Copy essential files into worktree from repo root.
    // Always overwrite: the repo root may have edits not yet committed to the branch,
    // and the worktree's git checkout would have a stale version.
    for filename in &["Dockerfile.devflow", ".env"] {
        let repo_file = git.root.join(filename);
        let worktree_file = worktree_path.join(filename);
        if repo_file.exists() {
            let _ = std::fs::copy(&repo_file, &worktree_file);
        }
    }

    // 5½b. Create common Rails tmp directories (gitignored, so missing in worktrees)
    for dir in &["tmp/pids", "tmp/cache", "tmp/sockets", "log"] {
        let _ = std::fs::create_dir_all(worktree_path.join(dir));
    }

    // Warn if .env exists but may not be gitignored
    let env_file = git.root.join(".env");
    if env_file.exists() {
        let gitignore = git.root.join(".gitignore");
        let is_ignored = std::fs::read_to_string(&gitignore)
            .map(|c| c.lines().any(|l| l.trim() == ".env"))
            .unwrap_or(false);
        if !is_ignored {
            eprintln!(
                "Warning: .env exists but is not in .gitignore — secrets may be committed!"
            );
        }
    }

    // 5a-5d. Optionally start compose stack
    let mut compose_file = None;
    let mut compose_ports = None;

    if enable_compose {
        // 5a. Check docker compose is available
        if let Err(e) = compose_mgr::check_available() {
            if !reusing_worktree {
                let _ = worktree::remove_worktree(&git.root, &worktree_path);
            }
            if branch_created {
                let _ = branch::delete_branch(git, branch_name);
            }
            return Err(e);
        }

        // 5b. Allocate ports
        let allocated = match ports::allocate(devflow_dir, task_name) {
            Ok(p) => p,
            Err(e) => {
                if !reusing_worktree {
                    let _ = worktree::remove_worktree(&git.root, &worktree_path);
                }
                if branch_created {
                    let _ = branch::delete_branch(git, branch_name);
                }
                return Err(e);
            }
        };

        // 5b½. Check ports are actually available on the host
        if let Err(e) = ports::check_ports_available(&allocated) {
            let _ = ports::release(devflow_dir, task_name);
            if !reusing_worktree {
                let _ = worktree::remove_worktree(&git.root, &worktree_path);
            }
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
                if !reusing_worktree {
                    let _ = worktree::remove_worktree(&git.root, &worktree_path);
                }
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
            if !reusing_worktree {
                let _ = worktree::remove_worktree(&git.root, &worktree_path);
            }
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
            if !reusing_worktree {
                let _ = worktree::remove_worktree(&git.root, &worktree_path);
            }
            if branch_created {
                let _ = branch::delete_branch(git, branch_name);
            }
            return Err(e);
        }

        // 5e½. Database setup (non-fatal: warn on failure, don't tear down)
        if db_clone {
            // Resolve source: explicit flag > config > auto-detect from worktree
            let source = if let Some(src) = db_source {
                src.to_string()
            } else {
                match compose_db::detect_source_db(&worktree_path) {
                    Ok(url) => {
                        println!("Auto-detected source database: {url}");
                        url
                    }
                    Err(e) => {
                        eprintln!("Warning: could not detect source database: {e}");
                        eprintln!("  Use --db-source to specify explicitly, or set compose_db_source in local.yml");
                        String::new()
                    }
                }
            };

            if !source.is_empty() {
                if let Err(e) = compose_db::clone_database(&cf, &source, task_name) {
                    eprintln!("Warning: database clone failed: {e}");
                    eprintln!("  The worker is running but the database may be empty.");
                    eprintln!("  You can retry with: devflow worker db-clone {task_name}");
                }
            }
        } else {
            compose_db::setup_database(&cf);
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

    // 5g. Generate CLAUDE.md in worktree (non-fatal)
    {
        let config_path = devflow_dir.join("config.yml");
        let project_name = ProjectConfig::load(&config_path)
            .map(|c| c.project_name)
            .unwrap_or_default();
        let detected_types = ProjectConfig::load(&config_path)
            .map(|c| c.detected_types.join(", "))
            .unwrap_or_default();
        let task_type = cli_task::load_tasks(devflow_dir)
            .ok()
            .and_then(|tasks| {
                tasks
                    .iter()
                    .find(|t| t.name == task_name)
                    .map(|t| t.task_type.clone())
            })
            .unwrap_or_default();

        let compose_file_str = compose_file
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let compose_project_str = compose_file
            .as_ref()
            .map(|p| compose_mgr::project_name(p))
            .unwrap_or_default();

        let vars = claude_md::ClaudeMdVars {
            worktree_path: &worktree_path.to_string_lossy(),
            worker_name: task_name,
            branch_name,
            project_name: &project_name,
            task_type: &task_type,
            detected_types: &detected_types,
            compose_enabled: compose_file.is_some(),
            compose_file: &compose_file_str,
            compose_project: &compose_project_str,
            app_port: compose_ports.as_ref().map(|p| p.app).unwrap_or(3000),
            db_port: compose_ports.as_ref().map(|p| p.db).unwrap_or(5432),
            redis_port: compose_ports.as_ref().map(|p| p.redis).unwrap_or(6379),
        };

        match claude_md::generate(&worktree_path, devflow_dir, &vars) {
            Ok(()) => println!("Generated CLAUDE.md in worktree"),
            Err(e) => eprintln!("Warning: failed to generate CLAUDE.md: {e}"),
        }
    }

    // 6. Create per-worker tmux workspace session
    let ws_template = workspace::load_template(devflow_dir)?
        .unwrap_or_else(workspace::default_template);

    let vars = workspace::WorkspaceVars {
        worktree_path: &worktree_path.to_string_lossy(),
        worker_name: task_name,
        app_port: compose_ports.as_ref().map(|p| p.app),
        db_port: compose_ports.as_ref().map(|p| p.db),
        redis_port: compose_ports.as_ref().map(|p| p.redis),
        compose_file: compose_file.as_deref(),
    };
    let rendered = workspace::render_template(&ws_template, &vars);
    let ws_name = workspace::worker_session_name(tmux_session, task_name);

    if let Err(e) = workspace::create_worker_session(&ws_name, &rendered, &worktree_path, compose_file.as_deref()) {
        workspace::destroy_worker_session(&ws_name);
        if let Some(ref cf) = compose_file {
            let _ = compose_mgr::down(cf);
            let _ = ports::release(devflow_dir, task_name);
            let compose_dir = devflow_dir.join("compose").join(task_name);
            let _ = std::fs::remove_dir_all(compose_dir);
        }
        if !reusing_worktree {
            let _ = worktree::remove_worktree(&git.root, &worktree_path);
        }
        if branch_created {
            let _ = branch::delete_branch(git, branch_name);
        }
        return Err(e);
    }

    // 7. Send initial command if provided
    if let Some(cmd) = initial_command {
        if let Some(first_win) = ws_template.windows.first() {
            let target = format!("{ws_name}:{}.0", first_win.name);
            if let Err(e) = session::send_keys_to_pane(&target, cmd) {
                eprintln!("Warning: failed to send initial command to workspace: {e}");
            }
        }
    }

    // 8. Save worker state
    let state = WorkerState {
        task_name: task_name.to_string(),
        branch: branch_name.to_string(),
        worktree_path: worktree_path.clone(),
        tmux_window: None,
        container_id: None,
        created_at: chrono::Utc::now(),
        pid: None,
        compose_file,
        compose_ports,
        tmux_session: Some(ws_name.clone()),
    };

    if let Err(e) = state.save(&state_path) {
        workspace::destroy_worker_session(&ws_name);
        if let Some(ref cf) = state.compose_file {
            let _ = compose_mgr::down(cf);
            let _ = ports::release(devflow_dir, task_name);
            let compose_dir = devflow_dir.join("compose").join(task_name);
            let _ = std::fs::remove_dir_all(compose_dir);
        }
        if !reusing_worktree {
            let _ = worktree::remove_worktree(&git.root, &worktree_path);
        }
        if branch_created {
            let _ = branch::delete_branch(git, branch_name);
        }
        return Err(e);
    }

    Ok(state)
}

/// Stop a worker: tear down ephemeral resources (compose, tmux, state) but keep worktree + branch.
pub fn stop(devflow_dir: &Path, task_name: &str) -> Result<()> {
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

    // Kill per-worker tmux session
    if let Some(ref ws) = state.tmux_session {
        workspace::destroy_worker_session(ws);
    }

    // Remove state file (but NOT worktree or branch)
    std::fs::remove_file(&state_path)?;

    // Remove lock file if it exists
    let lock_path = devflow_dir.join("locks").join(format!("{task_name}.lock"));
    let _ = std::fs::remove_file(lock_path);

    Ok(())
}

/// Kill a worker: tear down compose stack, remove tmux session, worktree, branch, and state file.
/// If the worktree has uncommitted changes or unpushed commits and `force` is false,
/// returns an error suggesting `stop` or `kill --force`.
pub fn kill(git: &GitRepo, devflow_dir: &Path, task_name: &str, force: bool) -> Result<()> {
    let state_path = WorkerState::state_path(devflow_dir, task_name);
    if !state_path.exists() {
        return Err(DevflowError::WorkerNotFound(task_name.to_string()));
    }

    let state = WorkerState::load(&state_path)?;

    // Check for dirty worktree before destroying
    if !force && state.worktree_path.exists() {
        let has_changes = worktree::has_uncommitted_changes(&state.worktree_path);
        let ahead = worktree::commits_ahead_of(&git.root, &state.branch, "main");

        if has_changes || ahead > 0 {
            let mut reasons = Vec::new();
            if has_changes {
                reasons.push("uncommitted changes".to_string());
            }
            if ahead > 0 {
                reasons.push(format!("{ahead} unpushed commit(s)"));
            }
            return Err(DevflowError::Other(format!(
                "Worker '{}' has {}.\n\
                 Use 'devflow worker stop {0}' to tear down containers/tmux but keep your work.\n\
                 Use 'devflow worker kill {0} --force' to destroy everything.",
                task_name,
                reasons.join(" and "),
            )));
        }
    }

    // Tear down compose stack if present
    if let Some(ref cf) = state.compose_file {
        if let Err(e) = compose_mgr::down(cf) {
            eprintln!("Warning: compose down failed: {e}");
        }
        let _ = ports::release(devflow_dir, task_name);
        let compose_dir = devflow_dir.join("compose").join(task_name);
        let _ = std::fs::remove_dir_all(compose_dir);
    }

    // Kill per-worker tmux session
    if let Some(ref ws) = state.tmux_session {
        workspace::destroy_worker_session(ws);
    }

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
