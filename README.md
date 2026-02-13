# devflow

Parallel AI-assisted development orchestrator. Run multiple isolated Claude Code instances on different tasks at the same time — each gets its own git worktree, branch, and tmux window.

The typical problem: you're waiting on Claude Code to finish a feature, but you have three more tasks queued up. With devflow, you spawn a worker per task and they all run in parallel, fully isolated from each other.

## Requirements

- **Rust** (for building)
- **Git** (worktree support requires at least one commit in the repo)
- **tmux** (worker windows live in a shared tmux session)
- **Docker** (optional — only needed for container features)
- **Docker Compose** (optional — only needed for `--compose` per-worker isolation)

## Installation

```bash
# From source
cargo install --path .

# Or just build it
cargo build --release
# Binary at ./target/release/devflow
```

## Quick Start

```bash
# 1. Initialize devflow in any git repo
cd your-project
devflow init
# => Creates .devflow/ directory, detects project type, writes config

# 2. Create tasks for what you want to work on
devflow task create add-auth -t feature -d "Add JWT authentication"
devflow task create fix-nav -t bugfix -d "Fix navbar on mobile"
devflow task create refactor-db -t refactor

# 3. Spawn workers — each gets its own worktree + tmux window
devflow worker spawn add-auth
devflow worker spawn fix-nav
devflow worker spawn refactor-db

# 4. Attach to the tmux session and work in parallel
devflow tmux attach
# => Three tmux windows, each cd'd into its own worktree
# => Run Claude Code in each window — fully isolated branches

# 5. When done with a task, tear it down
devflow worker kill add-auth
devflow task close add-auth
# => Removes worktree, branch, tmux window, and state files
```

## How It Works

When you run `devflow worker spawn <task>`, devflow:

1. Acquires a file lock to prevent races
2. Checks that 500MB+ of disk space is free
3. Creates (or reuses) a git branch: `<project>/<type>/<name>`
4. Creates a git worktree at `.devflow/worktrees/<task>/`
5. Opens a tmux window named `<task>` cd'd into the worktree
6. Saves worker state to `.devflow/workers/<task>.json`

If any step fails, everything rolls back in reverse order. No half-created state.

When you run `devflow worker kill <task>`, it tears down in reverse: kills the tmux window, removes the worktree, deletes the branch, and cleans up state files.

## Commands

### `devflow init`

Initialize devflow in the current git repository. Creates the `.devflow/` directory structure, auto-detects the project type (Rails, Node, React Native, Python, Rust, Go), and writes config files.

```bash
devflow init
# => ✓ Detected: rust
# => ✓ Initialized devflow for project 'myapp'
```

Run it once per project. Running it again is a no-op.

### `devflow detect`

Show what project types devflow detected in the current repo (without modifying anything).

```bash
devflow detect
# => ✓ Detected project types:
# =>   - rails
# =>   - node
```

Detection checks for: `Gemfile` + `config/routes.rb` (Rails), `package.json` (Node), `react-native` in package.json (React Native), `pyproject.toml`/`setup.py`/`requirements.txt` (Python), `Cargo.toml` (Rust), `go.mod` (Go).

### `devflow task`

Manage development tasks. Each task has a name, type, description, state, and associated git branch.

```bash
# Create a task — auto-creates a branch from HEAD
devflow task create my-feature
devflow task create my-feature -t bugfix -d "Fix the login page"
# Task types: feature (default), bugfix, refactor, chore

# List all tasks with their state
devflow task list
# => Tasks:
# =>   ● my-feature [created] (myapp/feature/my-feature)
# =>   ● fix-login [active] (myapp/bugfix/fix-login)

# Show full details for a task
devflow task show my-feature

# State transitions
devflow task pause my-feature    # active → paused
devflow task resume my-feature   # paused → active
devflow task complete my-feature # active → completed

# Close a task (cleans up branch + worktree)
devflow task close my-feature    # any state → closed
```

**Task states:** `created` → `active` → `paused` → `completed` → `closed`

Spawning a worker automatically moves the task to `active`. Closing a task deletes its branch and worktree if they still exist.

### `devflow worker`

Spawn and manage parallel development environments.

```bash
# Spawn a worker for a task
devflow worker spawn my-feature
# => ✓ Worker spawned for task 'my-feature'
# =>   Branch:   myapp/feature/my-feature
# =>   Worktree: /path/to/.devflow/worktrees/my-feature
# =>   Tmux:     devflow:my-feature

# Spawn with per-worker Docker Compose isolation (app + db + redis)
devflow worker spawn my-feature --compose
# => ✓ Worker spawned for task 'my-feature'
# =>   Branch:   myapp/feature/my-feature
# =>   Worktree: /path/to/.devflow/worktrees/my-feature
# =>   Tmux:     devflow:my-feature
# =>   Compose stack:
# =>     App:   http://localhost:3001
# =>     DB:    localhost:5433
# =>     Redis: localhost:6380

# List all active workers
devflow worker list
# => Active workers:
# =>   ● my-feature [running] branch:myapp/feature/my-feature [compose: 3001:5433:6380]

# Kill a worker and clean up all its resources
devflow worker kill my-feature
# => ✓ Worker 'my-feature' killed and resources cleaned up

# See uptime and resource info for all workers
devflow worker monitor
# => Worker Monitor
# => Session: devflow
# => Active: 2/4
# =>   ● my-feature (uptime: 1h 23m)
# =>   ● fix-login (uptime: 0h 45m)

# Manually clean up orphaned workers (containers, worktrees, state)
devflow worker cleanup
# => ! Found 1 orphaned worker(s):
# =>   ● old-task branch:myapp/feature/old-task [compose stack running]
# => ✓ Cleaned up 1 orphaned worker(s)
```

Workers auto-detect and clean up orphans (e.g., if a tmux window was manually closed) on spawn, list, and monitor. Use `devflow worker cleanup` for on-demand cleanup.

### `devflow worktree`

Inspect and maintain git worktrees managed by devflow.

```bash
# List all worktrees (including the main one)
devflow worktree list
# => Worktrees:
# =>   ● /path/to/project [main] (ok)
# =>   ● /path/to/.devflow/worktrees/my-feature [myapp/feature/my-feature] (ok)

# Check health of all worktrees
devflow worktree health
# => ✓ All 2 worktrees healthy

# Clean up stale worktree entries from git
devflow worktree prune
```

### `devflow tmux`

Manage tmux sessions — both the shared hub and per-worker workspace sessions.

```bash
# Attach to the devflow hub session
devflow tmux attach

# Attach to a worker's dedicated workspace session
devflow tmux attach-worker add-auth
# => Falls back to hub session if no workspace exists

# Show session status (hub + workspace sessions)
devflow tmux status
# => ✓ Hub session 'devflow' with 3 window(s):
# =>   - add-auth
# =>   - fix-nav
# =>   - refactor-db
# =>
# => Worker workspace sessions:
# =>   ● devflow-add-auth [active] (2 windows)

# Apply a layout to the current window
devflow tmux layout tiled
devflow tmux layout even-horizontal
devflow tmux layout even-vertical
devflow tmux layout main-horizontal
devflow tmux layout main-vertical

# Generate a default workspace template
devflow tmux init-template
# => ✓ Created tmux workspace template at .devflow/tmux-layout.json
```

The hub session is named `devflow` by default (configurable in `.devflow/local.yml`). It's created automatically when you spawn the first worker.

### Workspace Templates

Workspace templates let you define a multi-window, multi-pane tmux layout that gets created for each worker. This is useful when you need dedicated windows for logs, servers, editors, and shells.

**How it works:**
- **Hub session** (`devflow`) — one window per worker, always created (existing behavior)
- **Per-worker session** (`devflow-<task>`) — full workspace from template, only when `.devflow/tmux-layout.json` exists

Generate a starter template:

```bash
devflow tmux init-template
```

This creates `.devflow/tmux-layout.json` with a Rails development layout. Edit it to match your workflow:

```json
{
  "windows": [
    {
      "name": "server",
      "layout": "tiled",
      "panes": [
        { "command": "tail -f log/development.log" },
        { "command": "bundle exec puma -p {{APP_PORT}}" },
        { "command": "bundle exec sidekiq" },
        {}
      ]
    },
    {
      "name": "editor",
      "layout": "main-vertical",
      "panes": [
        { "command": "vim", "focus": true },
        {},
        { "command": "claude" },
        { "command": "rails console" }
      ]
    }
  ]
}
```

**Template variables** (replaced at spawn time):
| Variable | Value |
|----------|-------|
| `{{WORKTREE_PATH}}` | Absolute path to worker's git worktree |
| `{{WORKER_NAME}}` | Task name |
| `{{APP_PORT}}` | Allocated app port (with `--compose`) |
| `{{DB_PORT}}` | Allocated database port (with `--compose`) |
| `{{REDIS_PORT}}` | Allocated Redis port (with `--compose`) |

**Pane options:**
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | string | none | Command to run in the pane |
| `directory` | string | worktree path | Working directory for the pane |
| `focus` | bool | false | Whether to focus this pane after creation |

If the template file is absent, workers get the original single-window behavior.

### `devflow container`

Manage Docker containers for isolated development environments (requires Docker).

```bash
# Build a container image
devflow container build my-feature

# Start a container (sleep infinity + bind mount)
devflow container start my-feature

# List devflow-managed containers
devflow container list

# Open a shell in a running container
devflow container shell my-feature

# Stop and remove a container
devflow container stop my-feature
```

### `devflow containerize`

Interactive wizard for setting up a Dockerfile for your project.

```bash
devflow containerize
# => Container Setup Wizard
# => ? Select a container template
# =>   > Rails
# =>     React Native
# =>     Custom (Ubuntu base)
# => ? Write Dockerfile to project? (Y/n)
# => ✓ Wrote /path/to/Dockerfile.devflow
```

Writes a `Dockerfile.devflow` to your project root and enables container support in the config. The wizard also offers to generate a `compose-template.yml` for use with `--compose` per-worker stacks.

### `devflow commit`

Interactive conventional commit helper. Prompts for commit type, optional scope, and message.

```bash
devflow commit
# => Conventional Commit Helper
# => Staged changes:
# =>  src/auth.rs | 42 +++
# => ? Commit type
# =>   > feat: A new feature
# =>     fix: A bug fix
# =>     refactor: Code refactoring
# =>     ...
# => ? Scope (optional): auth
# => ? Short description: add JWT token validation
# => Commit message: feat(auth): add JWT token validation
# => ✓ Committed!
```

Stage your files with `git add` first, then run `devflow commit`.

## Configuration

### `.devflow/config.yml` — Project config (committed to git)

```yaml
project_name: myapp
detected_types:
- rails
- node
container_enabled: false
default_branch: main
```

### `.devflow/local.yml` — Local config (gitignored)

```yaml
tmux_session_name: devflow
max_workers: 4
min_disk_space_mb: 500
compose_health_timeout_secs: 60   # seconds to wait for containers to be ready (default: 60)
compose_post_start:               # commands to run in the "app" service after compose up
  - "bin/rails db:prepare"
  - "bin/rails assets:precompile"
```

## Project Layout

```
.devflow/
  config.yml          # Project configuration
  local.yml           # Local user config
  tasks.json          # Task database
  tmux-layout.json    # Workspace template (optional, for per-worker sessions)
  compose-template.yml # Docker Compose template (optional, for --compose)
  ports.json          # Port allocation registry (for --compose)
  worktrees/           # Git worktrees (one per worker)
    my-feature/        # Full checkout on its own branch
    fix-login/
  workers/             # Worker state files
    my-feature.json    # Branch, worktree path, tmux window, timestamps
    fix-login.json
  compose/             # Per-worker compose files (for --compose)
    my-feature/
      docker-compose.yml
  locks/               # File locks (prevent concurrent spawns)
    my-feature.lock
```

Everything under `.devflow/` is gitignored by default.

## Typical Workflow

```
┌─────────────────────────────────────────────────────────┐
│  devflow init                                           │
│  devflow task create feature-a                          │
│  devflow task create feature-b                          │
│  devflow task create bugfix-c                           │
│                                                         │
│  devflow worker spawn feature-a                         │
│  devflow worker spawn feature-b                         │
│  devflow worker spawn bugfix-c                          │
│                                                         │
│  devflow tmux attach                                    │
│  ┌──────────────┬──────────────┬──────────────┐         │
│  │ feature-a    │ feature-b    │ bugfix-c     │         │
│  │              │              │              │         │
│  │ claude code  │ claude code  │ claude code  │         │
│  │ (worktree a) │ (worktree b) │ (worktree c) │         │
│  │ (branch a)   │ (branch b)   │ (branch c)   │         │
│  └──────────────┴──────────────┴──────────────┘         │
│                                                         │
│  # When feature-a is done:                              │
│  devflow worker kill feature-a                          │
│  cd back-to-main && git merge feature-a-branch          │
│  devflow task close feature-a                           │
└─────────────────────────────────────────────────────────┘
```

## Branch Naming

Branches follow the convention: `<project>/<type>/<name>`

| Task | Branch |
|------|--------|
| `devflow task create auth -t feature` | `myapp/feature/auth` |
| `devflow task create nav -t bugfix` | `myapp/bugfix/nav` |
| `devflow task create db -t refactor` | `myapp/refactor/db` |

The project name comes from the directory name (set during `devflow init`).

## Safety

- **File locking** prevents two workers from spawning for the same task simultaneously
- **Disk space check** requires 500MB free before creating a worktree (configurable)
- **Atomic rollback** — if any step of worker spawn fails, all previous steps are reversed (including compose teardown and port release)
- **Port conflict pre-check** — before starting a compose stack, devflow verifies that allocated ports (app/db/redis) are actually free on the host; if a port is in use, you get a clear error instead of a cryptic Docker failure
- **Orphan cleanup** — workers whose tmux windows disappeared are detected and cleaned up automatically on spawn, list, monitor, and via `devflow worker cleanup`
- **Health check waiting** — after `compose up`, devflow polls container status until all services are running (and healthy, if a healthcheck is defined), with a configurable timeout (default 60s)
- **Post-start hooks** — run commands inside the `app` container after health checks pass (e.g., `db:prepare`); failures warn but don't tear down the stack
- **No force operations** — worktree removal uses `git worktree remove --force` but branch deletion is safe (won't delete unmerged branches that git protects)
- **Port allocation locking** — `ports.json` is protected by a file lock so concurrent `--compose` spawns never collide on ports
- **Clean compose teardown** — `worker kill` runs `docker compose down -v` to stop containers and remove volumes before cleaning up other resources
