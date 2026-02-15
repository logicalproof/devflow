use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{DevflowError, Result};

use super::manager as compose_mgr;

/// Check that `pg_dump` is available on the host PATH.
pub fn check_pg_dump_available() -> Result<()> {
    if which::which("pg_dump").is_err() {
        return Err(DevflowError::Other(
            "pg_dump not found on PATH. Install PostgreSQL client tools:\n  \
             macOS:  brew install libpq && brew link --force libpq\n  \
             Ubuntu: sudo apt-get install postgresql-client\n  \
             Arch:   sudo pacman -S postgresql-libs"
                .to_string(),
        ));
    }
    Ok(())
}

/// Detect the development database URL from the worktree's config/database.yml.
///
/// Parses the YAML looking for the `development:` section and extracts the
/// `database:` value. Handles ERB `<%= ... || "fallback" %>` patterns by
/// extracting the fallback string. Returns a postgres:// URL.
pub fn detect_source_db(worktree_path: &Path) -> Result<String> {
    let db_yml = worktree_path.join("config/database.yml");
    if !db_yml.exists() {
        return Err(DevflowError::Other(format!(
            "No config/database.yml found at {}",
            worktree_path.display()
        )));
    }

    let contents = std::fs::read_to_string(&db_yml)?;
    let mut in_development = false;
    let mut indent_level = None;

    for line in contents.lines() {
        let trimmed = line.trim();

        // Track top-level sections
        if !line.starts_with(' ') && !line.starts_with('\t') && trimmed.ends_with(':') {
            in_development = trimmed == "development:";
            indent_level = None;
            continue;
        }

        if !in_development {
            continue;
        }

        // Check for nested section start (e.g. "  <<: *default")
        if trimmed.starts_with("database:") || trimmed.starts_with("database :") {
            let leading_spaces = line.len() - line.trim_start().len();
            if let Some(level) = indent_level {
                if leading_spaces <= level {
                    // We've left the development section
                    break;
                }
            } else {
                indent_level = Some(leading_spaces);
            }

            // Extract the value after "database:"
            let value = trimmed
                .split_once(':')
                .map(|(_, v)| v.trim())
                .unwrap_or("");

            let db_name = extract_db_name(value);
            if !db_name.is_empty() {
                return Ok(format!("postgres://localhost:5432/{db_name}"));
            }
        }

        // Track indent level from first key in development section
        if indent_level.is_none() && !trimmed.is_empty() && !trimmed.starts_with('#') {
            let leading_spaces = line.len() - line.trim_start().len();
            indent_level = Some(leading_spaces);
        }
    }

    // Fallback: use directory name heuristic
    let dir_name = worktree_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if dir_name.is_empty() {
        return Err(DevflowError::Other(
            "Could not detect database name from config/database.yml".to_string(),
        ));
    }

    // Workers have names like "task-name", parent repo might be "Reportal"
    // Walk up to find the original repo name
    let repo_root = worktree_path.join(".git");
    let db_name = if repo_root.is_file() {
        // This is a worktree — read the gitdir link to find the parent repo name
        std::fs::read_to_string(&repo_root)
            .ok()
            .and_then(|content| {
                // Format: "gitdir: /path/to/repo/.git/worktrees/task-name"
                content.split('/').rev().nth(2).map(|s| s.to_string())
            })
            .unwrap_or(dir_name)
    } else {
        dir_name
    };

    Ok(format!(
        "postgres://localhost:5432/{}_development",
        db_name.replace('-', "_")
    ))
}

/// Extract a database name from a YAML value, handling ERB templates.
fn extract_db_name(value: &str) -> String {
    if value.contains("<%=") {
        // ERB template like: <%= ENV['DB_NAME'] || "Reportal_development" %>
        // Look for the fallback value after `||`
        let search_region = if let Some(idx) = value.find("||") {
            &value[idx..]
        } else {
            // No fallback operator — no static name to extract
            return String::new();
        };

        // Extract a double-quoted string
        if let Some(start) = search_region.find('"') {
            if let Some(end) = search_region[start + 1..].find('"') {
                return search_region[start + 1..start + 1 + end].to_string();
            }
        }
        // Extract a single-quoted string
        if let Some(start) = search_region.find('\'') {
            if let Some(end) = search_region[start + 1..].find('\'') {
                return search_region[start + 1..start + 1 + end].to_string();
            }
        }
        String::new()
    } else {
        value.trim_matches('"').trim_matches('\'').to_string()
    }
}

/// Run `rails db:prepare` and `rails db:seed` in the compose app container.
/// Non-fatal: prints warnings on failure.
pub fn setup_database(compose_file: &Path) {
    println!("Setting up database (db:prepare)...");
    match compose_mgr::exec(compose_file, "app", "rails db:prepare") {
        Ok(()) => println!("  Database prepared successfully."),
        Err(e) => {
            eprintln!("  Warning: db:prepare failed: {e}");
            eprintln!("  You can run it manually: docker compose exec app rails db:prepare");
            return; // Skip seed if prepare failed
        }
    }

    println!("Seeding database (db:seed)...");
    match compose_mgr::exec(compose_file, "app", "rails db:seed") {
        Ok(()) => println!("  Database seeded successfully."),
        Err(e) => {
            eprintln!("  Warning: db:seed failed: {e}");
            eprintln!("  You can run it manually: docker compose exec app rails db:seed");
        }
    }
}

/// Clone the host database into the worker's compose PostgreSQL container.
///
/// Pipes `pg_dump` from the host into `docker compose exec -T db psql` in
/// the container. Uses `--no-owner --no-acl --clean --if-exists` for a clean
/// restore.
pub fn clone_database(
    compose_file: &Path,
    source_url: &str,
    worker_name: &str,
) -> Result<()> {
    check_pg_dump_available()?;

    // Parse the source URL to extract connection info
    let (host, port, db_name) = parse_pg_url(source_url)?;

    // Pre-flight: verify the source database is reachable
    println!("Verifying source database '{db_name}' is reachable...");
    let check = Command::new("pg_dump")
        .args(["-h", &host, "-p", &port, "-d", &db_name, "--schema-only", "-t", "__devflow_preflight_nonexistent__"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;

    // pg_dump with a nonexistent table will fail, but if the *database* is unreachable
    // the error message will mention "could not connect" or "does not exist" (for the db).
    let check_stderr = String::from_utf8_lossy(&check.stderr);
    if check_stderr.contains("could not connect")
        || check_stderr.contains("does not exist")
        || check_stderr.contains("Connection refused")
        || check_stderr.contains("No such file or directory")
    {
        return Err(DevflowError::Other(format!(
            "Cannot connect to source database '{db_name}' at {host}:{port}.\n\
             Is PostgreSQL running? Check: pg_isready -h {host} -p {port}"
        )));
    }

    println!("Cloning database '{db_name}' into worker '{worker_name}'...");

    // Start pg_dump
    let mut pg_dump = Command::new("pg_dump")
        .args([
            "-h", &host,
            "-p", &port,
            "-d", &db_name,
            "--no-owner",
            "--no-acl",
            "--clean",
            "--if-exists",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| DevflowError::Other(format!("Failed to start pg_dump: {e}")))?;

    let pg_dump_stdout = pg_dump
        .stdout
        .take()
        .ok_or_else(|| DevflowError::Other("Failed to capture pg_dump stdout".to_string()))?;

    let pg_dump_stderr = pg_dump
        .stderr
        .take()
        .ok_or_else(|| DevflowError::Other("Failed to capture pg_dump stderr".to_string()))?;

    // Background thread to drain pg_dump stderr (prevents deadlock)
    let stderr_handle = std::thread::spawn(move || {
        let reader = BufReader::new(pg_dump_stderr);
        let mut errors = Vec::new();
        for line in reader.lines() {
            if let Ok(line) = line {
                if !line.is_empty() {
                    errors.push(line);
                }
            }
        }
        errors
    });

    // Start psql in the compose db container
    let project = compose_mgr::project_name(compose_file);
    let mut psql = Command::new("docker")
        .args([
            "compose",
            "-f", &compose_file.to_string_lossy(),
            "-p", &project,
            "exec", "-T",
            "db",
            "psql", "-U", "postgres", "-d", "app_development",
        ])
        .stdin(Stdio::from(pg_dump_stdout))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| DevflowError::Other(format!("Failed to start psql in container: {e}")))?;

    let psql_status = psql
        .wait()
        .map_err(|e| DevflowError::Other(format!("Failed to wait for psql: {e}")))?;

    let pg_dump_status = pg_dump
        .wait()
        .map_err(|e| DevflowError::Other(format!("Failed to wait for pg_dump: {e}")))?;

    // Collect stderr from pg_dump
    let dump_errors = stderr_handle.join().unwrap_or_default();
    let real_errors: Vec<&String> = dump_errors
        .iter()
        .filter(|l| !l.contains("NOTICE:") && !l.contains("WARNING:"))
        .collect();

    if !real_errors.is_empty() {
        eprintln!("  pg_dump messages:");
        for e in &real_errors {
            eprintln!("    {e}");
        }
    }

    // Collect psql output
    if let Some(mut psql_stderr) = psql.stderr.take() {
        let mut buf = String::new();
        use std::io::Read;
        let _ = psql_stderr.read_to_string(&mut buf);
        let psql_errors: Vec<&str> = buf
            .lines()
            .filter(|l| l.contains("ERROR"))
            .collect();
        if !psql_errors.is_empty() {
            eprintln!("  psql errors:");
            for e in &psql_errors {
                eprintln!("    {e}");
            }
        }
    }

    if !pg_dump_status.success() {
        return Err(DevflowError::Other(
            "pg_dump failed — check the errors above".to_string(),
        ));
    }

    if !psql_status.success() {
        // psql often returns non-zero for NOTICEs during --clean restore,
        // which is expected. Only fail if pg_dump itself failed.
        eprintln!("  Warning: psql exited with non-zero status (this may be normal for --clean restores)");
    }

    println!("  Database cloned successfully.");
    Ok(())
}

/// Parse a postgres:// URL into (host, port, db_name).
fn parse_pg_url(url: &str) -> Result<(String, String, String)> {
    let stripped = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
        .ok_or_else(|| {
            DevflowError::Other(format!(
                "Invalid database URL: '{url}'. Expected postgres://host:port/dbname"
            ))
        })?;

    // Format: [user:pass@]host[:port]/dbname
    let after_auth = if let Some(idx) = stripped.find('@') {
        &stripped[idx + 1..]
    } else {
        stripped
    };

    let (host_port, db_name) = after_auth.split_once('/').ok_or_else(|| {
        DevflowError::Other(format!(
            "Invalid database URL: '{url}'. Expected postgres://host:port/dbname"
        ))
    })?;

    let (host, port) = if let Some((h, p)) = host_port.split_once(':') {
        (h.to_string(), p.to_string())
    } else {
        (host_port.to_string(), "5432".to_string())
    };

    Ok((host, port, db_name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pg_url_basic() {
        let (host, port, db) = parse_pg_url("postgres://localhost:5432/mydb").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, "5432");
        assert_eq!(db, "mydb");
    }

    #[test]
    fn test_parse_pg_url_default_port() {
        let (host, port, db) = parse_pg_url("postgres://localhost/mydb").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, "5432");
        assert_eq!(db, "mydb");
    }

    #[test]
    fn test_parse_pg_url_with_auth() {
        let (host, port, db) = parse_pg_url("postgres://user:pass@db.host:5433/prod").unwrap();
        assert_eq!(host, "db.host");
        assert_eq!(port, "5433");
        assert_eq!(db, "prod");
    }

    #[test]
    fn test_parse_pg_url_postgresql_scheme() {
        let (host, port, db) = parse_pg_url("postgresql://localhost:5432/mydb").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, "5432");
        assert_eq!(db, "mydb");
    }

    #[test]
    fn test_parse_pg_url_invalid() {
        assert!(parse_pg_url("mysql://localhost/mydb").is_err());
        assert!(parse_pg_url("postgres://localhost").is_err());
    }

    #[test]
    fn test_extract_db_name_plain() {
        assert_eq!(extract_db_name("Reportal_development"), "Reportal_development");
    }

    #[test]
    fn test_extract_db_name_quoted() {
        assert_eq!(extract_db_name("\"Reportal_development\""), "Reportal_development");
    }

    #[test]
    fn test_extract_db_name_erb_double_quotes() {
        assert_eq!(
            extract_db_name("<%= ENV['DB_NAME'] || \"Reportal_development\" %>"),
            "Reportal_development"
        );
    }

    #[test]
    fn test_extract_db_name_erb_single_quotes() {
        assert_eq!(
            extract_db_name("<%= ENV['DB_NAME'] || 'Reportal_development' %>"),
            "Reportal_development"
        );
    }

    #[test]
    fn test_extract_db_name_erb_no_fallback() {
        assert_eq!(extract_db_name("<%= ENV['DB_NAME'] %>"), "");
    }
}
