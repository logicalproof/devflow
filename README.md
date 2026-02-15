# th (treehouse)

Parallel AI-assisted development orchestrator. Run multiple isolated Claude Code instances on different tasks at the same time — each gets its own git worktree, branch, and tmux window.

The typical problem: you're waiting on Claude Code to finish a feature, but you have three more tasks queued up. With `th`, you plant a grove or tree per task and they all run in parallel, fully isolated from each other.

- **Grove** — containerized environment (worktree + Docker Compose stack + tmux)
- **Tree** — lightweight worktree only (worktree + tmux, no containers)

## Requirements

- **Rust** (for building)
- **Git** (worktree support requires at least one commit in the repo)
- **tmux** (worker windows live in a shared tmux session)
- **Docker** (optional — only needed for `grove` commands)
- **Docker Compose** (optional — only needed for `grove` per-task isolation)
- **pg_dump** (optional — only needed for `--transplant`; install via `brew install libpq` on macOS)

## Installation

```bash
# From source
cargo install --path .

# Or just build it
cargo build --release
# Binary at ./target/release/th
```

## Quick Start

```bash
# 1. Initialize treehouse in any git repo
cd your-project
th init
# => Creates .treehouse/ directory, detects project type, writes config

# 2. Plant groves (containerized) or trees (lightweight)
th grove plant add-auth -t feature           # full Docker Compose stack
th tree plant fix-nav -t bugfix              # just a worktree + tmux
th grove plant refactor-db -t refactor --transplant  # containerized + db clone

# 3. Attach to a session and work in parallel
th grove attach add-auth
# => Attached to tmux session with worktree, containers running

# 4. When done, stop or uproot
th grove stop add-auth
# => Tears down containers/tmux but keeps worktree + branch
# => Re-plant later with: th grove plant add-auth

th grove uproot add-auth
# => Removes everything: worktree, branch, tmux, containers, state
# => Refuses if there are uncommitted changes (use --force to override)
```

## How It Works

When you run `th grove plant <name>`, treehouse:

1. Acquires a file lock to prevent races
2. Checks that 500MB+ of disk space is free
3. Creates (or reuses) a git branch: `<project>/<type>/<name>`
4. Creates a git worktree at `.treehouse/worktrees/<task>/`
5. Generates a Docker Compose stack (app + db + redis) with unique ports
6. Waits for containers to be healthy
7. Opens a tmux window named `<task>` cd'd into the worktree
8. Saves state to `.treehouse/groves/<name>.json`

`th tree plant <name>` does steps 1–4 and 7–8, skipping containers entirely.

If any step fails, everything rolls back in reverse order. No half-created state.

## Commands

### `th init`

Initialize treehouse in the current git repository. Creates the `.treehouse/` directory structure, auto-detects the project type (Rails, Node, React Native, Python, Rust, Go), and writes config files.

```bash
th init
# => Detected: rust
# => Initialized treehouse for project 'myapp'
```

Run it once per project. Running it again is a no-op.

### `th detect`

Show what project types treehouse detected in the current repo (without modifying anything).

```bash
th detect
# => Detected project types:
# =>   - rails
# =>   - node
```

### `th grove`

Containerized development environments. Each grove gets its own worktree, Docker Compose stack (app + db + redis), and tmux session.

```bash
# Plant a grove (creates branch + worktree + compose stack)
th grove plant my-feature
th grove plant my-feature -t bugfix
# => Grove planted for task 'my-feature'
# =>   Branch:   myapp/feature/my-feature
# =>   Worktree: /path/to/.treehouse/worktrees/my-feature
# =>   Compose stack:
# =>     App:   http://localhost:3001
# =>     DB:    localhost:5433
# =>     Redis: localhost:6380

# Plant with database clone from host
th grove plant my-feature --transplant
# => Auto-detects source database from config/database.yml
# => Pipes pg_dump from host into the container's PostgreSQL

# Plant with a specific database source
th grove plant my-feature --transplant --db-source postgres://localhost:5432/myapp_dev

# Plant with a Claude prompt
th grove plant my-feature --prompt "Implement JWT authentication"
th grove plant my-feature --prompt-file tasks/auth-spec.md

# List all groves
th grove list
# => Active groves:
# =>   my-feature [ok] branch:myapp/feature/my-feature [compose: 3001:5433:6380]

# Show status and resource usage
th grove status

# Stop a grove (free containers/tmux, keep worktree + branch)
th grove stop my-feature
# => Re-plant with: th grove plant my-feature

# Start a stopped grove's containers
th grove start my-feature

# Uproot a grove and destroy all resources
th grove uproot my-feature
# => Refuses if worktree has uncommitted changes or unpushed commits
th grove uproot my-feature --force

# Clone host database into a running grove
th grove transplant my-feature
th grove transplant my-feature --db-source postgres://localhost:5432/myapp_dev

# Attach to a grove's tmux session
th grove attach my-feature
th grove attach              # attaches to first grove

# Rebuild container image
th grove build my-feature

# Clean up orphaned groves
th grove prune

# Tmux layout management
th grove layout tiled
th grove layout even-horizontal

# Generate templates
th grove init-template         # tmux workspace template
th grove init-claude-template  # CLAUDE.local.md template
```

### `th tree`

Lightweight worktrees — no containers, just a git worktree and tmux session.

```bash
# Plant a tree (creates branch + worktree, no containers)
th tree plant my-bugfix
th tree plant my-bugfix -t bugfix
# => Tree planted for task 'my-bugfix'
# =>   Branch:   myapp/bugfix/my-bugfix
# =>   Worktree: /path/to/.treehouse/worktrees/my-bugfix

# Plant with a Claude prompt
th tree plant my-bugfix --prompt "Fix the navbar collapse on mobile"

# List all trees
th tree list

# Show tree status
th tree status

# Stop a tree (tear down tmux, keep worktree)
th tree stop my-bugfix

# Uproot a tree (remove worktree + branch + tmux)
th tree uproot my-bugfix
th tree uproot my-bugfix --force

# Attach to a tree's tmux session
th tree attach my-bugfix
th tree attach               # attaches to first tree

# Maintenance
th tree prune                # clean up stale worktrees
th tree health               # check worktree health
```

### `th containerize`

Interactive wizard for setting up a Dockerfile for your project.

```bash
th containerize
# => Container Setup Wizard
# => ? Select a container template
# =>   > Rails
# =>     React Native
# =>     Custom (Ubuntu base)
# => ? Write Dockerfile to project? (Y/n)
# => Wrote /path/to/Dockerfile.dev
```

Writes a `Dockerfile.dev` to your project root. The wizard also offers to generate a `compose-template.yml` for use with grove stacks.

If your project already has a `Dockerfile.dev`, treehouse will use it directly. The default compose template references `Dockerfile.dev` and includes health-checked PostgreSQL and Redis services, with named volumes for bundle cache and node_modules.

### `th commit`

Interactive conventional commit helper. Prompts for commit type, optional scope, and message.

```bash
th commit
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
# => Committed!
```

Stage your files with `git add` first, then run `th commit`.

### Workspace Templates

Workspace templates let you define a multi-window, multi-pane tmux layout that gets created for each grove. This is useful when you need dedicated windows for logs, servers, editors, and shells.

**How it works:**
- **Hub session** (`treehouse`) — one window per grove/tree, always created
- **Per-grove session** (`treehouse-<task>`) — full workspace from template, only when `.treehouse/tmux-layout.json` exists

Generate a starter template:

```bash
th grove init-template
```

This creates `.treehouse/tmux-layout.json` with a Rails development layout. Edit it to match your workflow:

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

**Template variables** (replaced at plant time):
| Variable | Value |
|----------|-------|
| `{{WORKTREE_PATH}}` | Absolute path to the git worktree |
| `{{WORKER_NAME}}` | Task name |
| `{{APP_PORT}}` | Allocated app port (groves only) |
| `{{DB_PORT}}` | Allocated database port (groves only) |
| `{{REDIS_PORT}}` | Allocated Redis port (groves only) |

**Pane options:**
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `command` | string | none | Command to run in the pane |
| `directory` | string | worktree path | Working directory for the pane |
| `focus` | bool | false | Whether to focus this pane after creation |

If the template file is absent, groves/trees get the default single-window behavior.

## Configuration

### `.treehouse/config.yml` — Project config (committed to git)

```yaml
project_name: myapp
detected_types:
- rails
- node
container_enabled: false
default_branch: main
```

### `.treehouse/local.yml` — Local config (gitignored)

```yaml
tmux_session_name: treehouse
max_workers: 4
min_disk_space_mb: 500
compose_health_timeout_secs: 60   # seconds to wait for containers to be ready (default: 60)
compose_post_start:               # commands to run in the "app" service after compose up
  - "bin/rails db:prepare"
  - "bin/rails assets:precompile"
```

## Project Layout

```
.treehouse/
  config.yml          # Project configuration
  local.yml           # Local user config
  tmux-layout.json    # Workspace template (optional, for per-grove sessions)
  compose-template.yml # Docker Compose template (optional, for groves)
  ports.json          # Port allocation registry (for groves)
  worktrees/           # Git worktrees (one per grove/tree)
    my-feature/        # Full checkout on its own branch
    fix-login/
  groves/              # Grove/tree state files
    my-feature.json    # Branch, worktree path, tmux window, timestamps
    fix-login.json
  compose/             # Per-grove compose files
    my-feature/
      docker-compose.yml
  locks/               # File locks (prevent concurrent plants)
    my-feature.lock
```

Everything under `.treehouse/` is gitignored by default.

## Typical Workflow

```
┌─────────────────────────────────────────────────────────┐
│  th init                                                 │
│                                                          │
│  th grove plant feature-a --transplant                   │
│  th grove plant feature-b                                │
│  th tree plant bugfix-c -t bugfix                        │
│                                                          │
│  th grove attach feature-a                               │
│  ┌──────────────┬──────────────┬──────────────┐          │
│  │ feature-a    │ feature-b    │ bugfix-c     │          │
│  │              │              │              │          │
│  │ claude code  │ claude code  │ claude code  │          │
│  │ (grove)      │ (grove)      │ (tree)       │          │
│  │ (containers) │ (containers) │ (worktree)   │          │
│  └──────────────┴──────────────┴──────────────┘          │
│                                                          │
│  # When feature-a is done:                               │
│  th grove stop feature-a   # keep work, free resources   │
│  th grove uproot feature-a # or destroy everything       │
│  cd back-to-main && git merge feature-a-branch           │
└──────────────────────────────────────────────────────────┘
```

## Branch Naming

Branches follow the convention: `<project>/<type>/<name>`

| Command | Branch |
|---------|--------|
| `th grove plant auth` | `myapp/feature/auth` |
| `th tree plant nav -t bugfix` | `myapp/bugfix/nav` |
| `th grove plant db -t refactor` | `myapp/refactor/db` |

The project name comes from the directory name (set during `th init`).

## Safety

- **File locking** prevents two groves from being planted for the same task simultaneously
- **Disk space check** requires 500MB free before creating a worktree (configurable)
- **Atomic rollback** — if any step of planting fails, all previous steps are reversed (including compose teardown and port release)
- **Port conflict pre-check** — before starting a compose stack, treehouse verifies that allocated ports (app/db/redis) are actually free on the host; if a port is in use, you get a clear error instead of a cryptic Docker failure
- **Orphan cleanup** — groves whose tmux windows disappeared are detected and cleaned up automatically on plant, list, status, and via `th grove prune`
- **Health check waiting** — after `compose up`, treehouse polls container status until all services are running (and healthy, if a healthcheck is defined), with a configurable timeout (default 60s)
- **Post-start hooks** — run commands inside the `app` container after health checks pass (e.g., `db:prepare`); failures warn but don't tear down the stack
- **Dirty worktree protection** — `uproot` checks for uncommitted changes and unpushed commits before destroying a worktree; use `stop` to free resources while preserving work, or `uproot --force` to override
- **Port allocation locking** — `ports.json` is protected by a file lock so concurrent grove plants never collide on ports
- **Clean compose teardown** — `uproot` runs `docker compose down -v` to stop containers and remove volumes before cleaning up other resources
