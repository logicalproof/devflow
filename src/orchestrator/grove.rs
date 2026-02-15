use std::path::Path;
use std::time::Duration;

use sysinfo::Disks;

use crate::claude_md;
use crate::compose::{db as compose_db, manager as compose_mgr, ports};
use crate::config::lock::FileLock;
use crate::config::project::ProjectConfig;
use crate::error::{GrootError, Result};
use crate::git::{branch, repo::GitRepo, worktree};
use crate::tmux::{session, workspace};

use super::state::GroveState;

/// Plant a new grove/tree: create branch, worktree, optionally start compose stack,
/// create tmux workspace, save state.
pub fn plant(
    git: &GitRepo,
    groot_dir: &Path,
    task_name: &str,
    branch_name: &str,
    task_type: &str,
    tmux_session: &str,
    min_disk_mb: u64,
    initial_command: Option<&str>,
    enable_compose: bool,
    compose_health_timeout_secs: u64,
    compose_post_start: &[String],
    db_clone: bool,
    db_source: Option<&str>,
    shared_grove: Option<&str>,
    shared_compose_ports: Option<&ports::AllocatedPorts>,
) -> Result<GroveState> {
    // 1. Acquire lock
    let lock_path = groot_dir.join("locks").join(format!("{task_name}.lock"));
    let _lock = FileLock::acquire(&lock_path)?;

    // 2. Check for duplicate
    let state_path = GroveState::state_path(groot_dir, task_name);
    if state_path.exists() {
        return Err(GrootError::GroveAlreadyExists(task_name.to_string()));
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
    let worktree_path = groot_dir.join("worktrees").join(task_name);
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
    for filename in &["Dockerfile.dev", "Dockerfile.groot", ".env", "config/master.key"] {
        let repo_file = git.root.join(filename);
        let worktree_file = worktree_path.join(filename);
        if repo_file.exists() {
            if let Some(parent) = worktree_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&repo_file, &worktree_file);
        }
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
        let allocated = match ports::allocate(groot_dir, task_name) {
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
            let _ = ports::release(groot_dir, task_name);
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
            groot_dir,
            task_name,
            &worktree_path,
            &allocated,
        ) {
            Ok(cf) => cf,
            Err(e) => {
                let _ = ports::release(groot_dir, task_name);
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
            let _ = ports::release(groot_dir, task_name);
            let compose_dir = groot_dir.join("compose").join(task_name);
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
            let _ = ports::release(groot_dir, task_name);
            let compose_dir = groot_dir.join("compose").join(task_name);
            let _ = std::fs::remove_dir_all(compose_dir);
            if !reusing_worktree {
                let _ = worktree::remove_worktree(&git.root, &worktree_path);
            }
            if branch_created {
                let _ = branch::delete_branch(git, branch_name);
            }
            return Err(e);
        }

        // 5e¾. Database setup (non-fatal: warn on failure, don't tear down)
        if db_clone {
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
                    eprintln!("  The grove is running but the database may be empty.");
                    eprintln!("  You can retry with: groot grove transplant {task_name}");
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

    // 5g. Generate CLAUDE.local.md in worktree (non-fatal)
    {
        let config_path = groot_dir.join("config.yml");
        let project_name = ProjectConfig::load(&config_path)
            .map(|c| c.project_name)
            .unwrap_or_default();
        let detected_types = ProjectConfig::load(&config_path)
            .map(|c| c.detected_types.join(", "))
            .unwrap_or_default();

        let is_shared = shared_grove.is_some();
        let compose_file_str = compose_file
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let compose_project_str = compose_file
            .as_ref()
            .map(|p| compose_mgr::project_name(p))
            .unwrap_or_default();

        // Port values: prefer shared_compose_ports, then own compose_ports, then defaults
        let effective_ports = shared_compose_ports.or(compose_ports.as_ref());

        let vars = claude_md::ClaudeMdVars {
            worktree_path: &worktree_path.to_string_lossy(),
            worker_name: task_name,
            branch_name,
            project_name: &project_name,
            task_type,
            detected_types: &detected_types,
            compose_enabled: compose_file.is_some() && !is_shared,
            compose_file: &compose_file_str,
            compose_project: &compose_project_str,
            app_port: effective_ports.map(|p| p.app).unwrap_or(3000),
            db_port: effective_ports.map(|p| p.db).unwrap_or(5432),
            redis_port: effective_ports.map(|p| p.redis).unwrap_or(6379),
            shared_compose: is_shared,
            shared_grove_name: shared_grove.unwrap_or(""),
        };

        match claude_md::generate(&worktree_path, groot_dir, &vars) {
            Ok(()) => println!("Generated CLAUDE.local.md in worktree"),
            Err(e) => eprintln!("Warning: failed to generate CLAUDE.local.md: {e}"),
        }
    }

    // 6. Create per-grove tmux workspace session
    let ws_template = workspace::load_template(groot_dir)?
        .unwrap_or_else(workspace::default_template);

    // When sharing a grove's compose, use shared ports for template vars but don't
    // pass compose_file so panes run commands locally instead of via `docker compose exec`.
    let effective_ports = shared_compose_ports.or(compose_ports.as_ref());
    let effective_compose_file = if shared_grove.is_some() {
        None
    } else {
        compose_file.as_deref()
    };

    let vars = workspace::WorkspaceVars {
        worktree_path: &worktree_path.to_string_lossy(),
        worker_name: task_name,
        app_port: effective_ports.map(|p| p.app),
        db_port: effective_ports.map(|p| p.db),
        redis_port: effective_ports.map(|p| p.redis),
        compose_file: effective_compose_file,
    };
    let rendered = workspace::render_template(&ws_template, &vars);
    let ws_name = workspace::worker_session_name(tmux_session, task_name);

    if let Err(e) = workspace::create_worker_session(&ws_name, &rendered, &worktree_path, effective_compose_file) {
        workspace::destroy_worker_session(&ws_name);
        if let Some(ref cf) = compose_file {
            let _ = compose_mgr::down(cf);
            let _ = ports::release(groot_dir, task_name);
            let compose_dir = groot_dir.join("compose").join(task_name);
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

    // 8. Save state
    let state = GroveState {
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
        shared_grove: shared_grove.map(|s| s.to_string()),
        shared_compose_ports: shared_compose_ports.cloned(),
    };

    if let Err(e) = state.save(&state_path) {
        workspace::destroy_worker_session(&ws_name);
        if let Some(ref cf) = state.compose_file {
            let _ = compose_mgr::down(cf);
            let _ = ports::release(groot_dir, task_name);
            let compose_dir = groot_dir.join("compose").join(task_name);
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

/// Find trees that share a grove's compose stack.
fn find_sharing_trees(groot_dir: &Path, grove_name: &str) -> Vec<String> {
    let groves = list_groves(groot_dir).unwrap_or_default();
    groves
        .iter()
        .filter(|g| g.shared_grove.as_deref() == Some(grove_name))
        .map(|g| g.task_name.clone())
        .collect()
}

/// Stop a grove/tree: tear down ephemeral resources (compose, tmux, state) but keep worktree + branch.
pub fn stop(groot_dir: &Path, task_name: &str, force: bool) -> Result<()> {
    let state_path = GroveState::state_path(groot_dir, task_name);
    if !state_path.exists() {
        return Err(GrootError::GroveNotFound(task_name.to_string()));
    }

    let state = GroveState::load(&state_path)?;

    // Block or auto-stop sharing trees
    if state.compose_file.is_some() {
        let sharing = find_sharing_trees(groot_dir, task_name);
        if !sharing.is_empty() {
            if !force {
                return Err(GrootError::Other(format!(
                    "Grove '{}' has sharing tree(s): {}. Stop or uproot them first, or use --force to override.",
                    task_name,
                    sharing.join(", ")
                )));
            }
            for tree_name in &sharing {
                eprintln!("Stopping sharing tree '{tree_name}'...");
                if let Err(e) = stop(groot_dir, tree_name, false) {
                    eprintln!("Warning: failed to stop sharing tree '{tree_name}': {e}");
                }
            }
        }
    }

    // Tear down compose stack if present
    if let Some(ref cf) = state.compose_file {
        if let Err(e) = compose_mgr::down(cf) {
            eprintln!("Warning: compose down failed: {e}");
        }
        let _ = ports::release(groot_dir, task_name);
        let compose_dir = groot_dir.join("compose").join(task_name);
        let _ = std::fs::remove_dir_all(compose_dir);
    }

    // Kill per-grove tmux session
    if let Some(ref ws) = state.tmux_session {
        workspace::destroy_worker_session(ws);
    }

    // Remove state file (but NOT worktree or branch)
    std::fs::remove_file(&state_path)?;

    // Remove lock file if it exists
    let lock_path = groot_dir.join("locks").join(format!("{task_name}.lock"));
    let _ = std::fs::remove_file(lock_path);

    Ok(())
}

/// Uproot a grove/tree: tear down compose stack, remove tmux session, worktree, branch, and state file.
/// If the worktree has uncommitted changes or unpushed commits and `force` is false,
/// returns an error suggesting `stop` or `uproot --force`.
pub fn uproot(git: &GitRepo, groot_dir: &Path, task_name: &str, force: bool) -> Result<()> {
    let state_path = GroveState::state_path(groot_dir, task_name);
    if !state_path.exists() {
        return Err(GrootError::GroveNotFound(task_name.to_string()));
    }

    let state = GroveState::load(&state_path)?;

    let kind = if state.compose_file.is_some() { "grove" } else { "tree" };

    // Block or auto-stop sharing trees
    if state.compose_file.is_some() {
        let sharing = find_sharing_trees(groot_dir, task_name);
        if !sharing.is_empty() {
            if !force {
                return Err(GrootError::Other(format!(
                    "Grove '{}' has sharing tree(s): {}. Stop or uproot them first, or use --force to override.",
                    task_name,
                    sharing.join(", ")
                )));
            }
            for tree_name in &sharing {
                eprintln!("Stopping sharing tree '{tree_name}'...");
                if let Err(e) = stop(groot_dir, tree_name, false) {
                    eprintln!("Warning: failed to stop sharing tree '{tree_name}': {e}");
                }
            }
        }
    }

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
            return Err(GrootError::Other(format!(
                "{kind} '{}' has {}.\n\
                 Use 'groot {kind} stop {0}' to stop but keep your work.\n\
                 Use 'groot {kind} uproot {0} --force' to destroy everything.",
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
        let _ = ports::release(groot_dir, task_name);
        let compose_dir = groot_dir.join("compose").join(task_name);
        let _ = std::fs::remove_dir_all(compose_dir);
    }

    // Kill per-grove tmux session
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
    let lock_path = groot_dir.join("locks").join(format!("{task_name}.lock"));
    let _ = std::fs::remove_file(lock_path);

    Ok(())
}

/// List all groves from state files
pub fn list_groves(groot_dir: &Path) -> Result<Vec<GroveState>> {
    let groves_dir = groot_dir.join("groves");
    if !groves_dir.exists() {
        return Ok(Vec::new());
    }

    let mut groves = Vec::new();
    for entry in std::fs::read_dir(&groves_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            if let Ok(state) = GroveState::load(&path) {
                groves.push(state);
            }
        }
    }

    Ok(groves)
}

/// Get a grove by name
pub fn get_grove_by_name(groot_dir: &Path, task_name: &str) -> Result<GroveState> {
    let state_path = GroveState::state_path(groot_dir, task_name);
    if !state_path.exists() {
        return Err(GrootError::GroveNotFound(task_name.to_string()));
    }
    GroveState::load(&state_path)
}

fn check_disk_space(min_mb: u64) -> Result<()> {
    let disks = Disks::new_with_refreshed_list();
    for disk in disks.list() {
        if disk.mount_point() == Path::new("/") {
            let available_mb = disk.available_space() / (1024 * 1024);
            if available_mb < min_mb {
                return Err(GrootError::InsufficientDiskSpace {
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
