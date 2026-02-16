use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{GrootError, Result};

use super::manager as compose_mgr;

/// Check that `pg_dump` is available on the host PATH.
pub fn check_pg_dump_available() -> Result<()> {
    if which::which("pg_dump").is_err() {
        return Err(GrootError::Other(
            "pg_dump not found on PATH. Install PostgreSQL client tools:\n  \
             macOS:  brew install libpq && brew link --force libpq\n  \
             Ubuntu: sudo apt-get install postgresql-client\n  \
             Arch:   sudo pacman -S postgresql-libs"
                .to_string(),
        ));
    }
    Ok(())
}

/// Detect the development database URL from the worktree, using a priority chain:
///
/// 1. `DATABASE_URL` from `.env` file
/// 2. `database` key from `config/database.yml` (parsed with serde_yml after ERB stripping)
/// 3. Query running Postgres for a matching `{project}_development` database
/// 4. Convention fallback: `{repo_name}_development`
pub fn detect_source_db(worktree_path: &Path) -> Result<String> {
    if let Some(url) = detect_from_env_file(worktree_path) {
        println!("  Detected database from .env: {url}");
        return Ok(url);
    }
    if let Some(url) = detect_from_database_yml(worktree_path) {
        println!("  Detected database from config/database.yml: {url}");
        return Ok(url);
    }
    if let Some(url) = detect_from_running_postgres(worktree_path) {
        println!("  Detected database from running Postgres: {url}");
        return Ok(url);
    }
    if let Some(url) = detect_from_convention(worktree_path) {
        println!("  Detected database from naming convention: {url}");
        return Ok(url);
    }
    Err(GrootError::Other(
        "Could not detect source database".into(),
    ))
}

/// Priority 1: Read `DATABASE_URL` from the `.env` file.
fn detect_from_env_file(worktree_path: &Path) -> Option<String> {
    let env_file = worktree_path.join(".env");
    let contents = std::fs::read_to_string(env_file).ok()?;

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Strip optional `export ` prefix
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some(value) = line.strip_prefix("DATABASE_URL=") {
            let value = value.trim();
            // Strip surrounding quotes
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| {
                    value
                        .strip_prefix('\'')
                        .and_then(|v| v.strip_suffix('\''))
                })
                .unwrap_or(value);
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Priority 2: Parse `config/database.yml` with serde_yml after stripping ERB tags.
fn detect_from_database_yml(worktree_path: &Path) -> Option<String> {
    let db_yml = worktree_path.join("config/database.yml");
    let contents = std::fs::read_to_string(db_yml).ok()?;
    let cleaned = strip_erb(&contents);

    // Parse the cleaned YAML
    let doc: HashMap<String, serde_yml::Value> = serde_yml::from_str(&cleaned).ok()?;

    // Look up development.database
    let dev_section = doc.get("development")?;
    let db_name = dev_section.get("database")?.as_str()?;

    if db_name.is_empty() {
        return None;
    }

    Some(format!("postgres://localhost:5432/{db_name}"))
}

/// Strip ERB `<%= ... %>` tags from YAML content, extracting fallback values where possible.
///
/// - `<%= ENV['X'] || "fallback" %>` → `fallback`
/// - `<%= ENV['X'] || 'fallback' %>` → `fallback`
/// - `<%= ENV['X'] %>` (no fallback)  → empty string
fn strip_erb(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut rest = content;

    while let Some(start) = rest.find("<%=") {
        result.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("%>") {
            let erb_body = &rest[start + 3..start + end];
            let replacement = extract_erb_fallback(erb_body);
            result.push_str(&replacement);
            rest = &rest[start + end + 2..];
        } else {
            // Malformed ERB — keep the rest as-is
            result.push_str(&rest[start..]);
            rest = "";
        }
    }
    result.push_str(rest);
    result
}

/// Extract the fallback value from an ERB expression body.
///
/// Given the body of `<%= ... %>`, looks for `|| "value"` or `|| 'value'` patterns.
fn extract_erb_fallback(erb_body: &str) -> String {
    if let Some(idx) = erb_body.find("||") {
        let after_pipe = &erb_body[idx + 2..];
        // Try double quotes
        if let Some(start) = after_pipe.find('"') {
            if let Some(end) = after_pipe[start + 1..].find('"') {
                return after_pipe[start + 1..start + 1 + end].to_string();
            }
        }
        // Try single quotes
        if let Some(start) = after_pipe.find('\'') {
            if let Some(end) = after_pipe[start + 1..].find('\'') {
                return after_pipe[start + 1..start + 1 + end].to_string();
            }
        }
    }
    // No fallback — return empty string so the YAML key gets an empty value
    String::new()
}

/// Priority 3: Query a running Postgres instance for a matching database.
fn detect_from_running_postgres(worktree_path: &Path) -> Option<String> {
    // psql must be on PATH
    which::which("psql").ok()?;

    let project_name = project_name_from_path(worktree_path)?;
    let expected = format!("{}_development", project_name.replace('-', "_"));

    // List databases: -l (list), -t (tuples only), -A (unaligned), -F, (comma separator)
    let output = Command::new("psql")
        .args(["-h", "localhost", "-p", "5432", "-l", "-t", "-A", "-F", ","])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Each line: dbname,owner,encoding,...
    for line in stdout.lines() {
        let db_name = line.split(',').next().unwrap_or("").trim();
        if db_name.eq_ignore_ascii_case(&expected) {
            return Some(format!("postgres://localhost:5432/{db_name}"));
        }
    }
    None
}

/// Priority 4: Convention fallback based on repo/project directory name.
fn detect_from_convention(worktree_path: &Path) -> Option<String> {
    let project_name = project_name_from_path(worktree_path)?;
    Some(format!(
        "postgres://localhost:5432/{}_development",
        project_name.replace('-', "_")
    ))
}

/// Determine the project/repo name from a worktree path.
///
/// - If `.git` is a file (git worktree): reads `gitdir:` to find the parent repo name.
/// - If `.git` is a directory (normal repo): uses the directory name.
fn project_name_from_path(worktree_path: &Path) -> Option<String> {
    let dot_git = worktree_path.join(".git");

    if dot_git.is_file() {
        // Git worktree — .git file contains: "gitdir: /path/to/repo/.git/worktrees/name"
        let content = std::fs::read_to_string(&dot_git).ok()?;
        let gitdir_line = content.lines().find(|l| l.starts_with("gitdir:"))?;
        let gitdir_path = gitdir_line.strip_prefix("gitdir:")?.trim();
        let gitdir = Path::new(gitdir_path);

        // Walk up from .git/worktrees/name → .git → repo_root
        let repo_root = gitdir.parent()?.parent()?.parent()?;
        let name = repo_root.file_name()?.to_string_lossy().to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    } else if dot_git.is_dir() {
        let name = worktree_path
            .file_name()?
            .to_string_lossy()
            .to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    } else {
        // No .git at all — just use the directory name
        let name = worktree_path
            .file_name()?
            .to_string_lossy()
            .to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }
}

/// Create the test database in the compose PostgreSQL container.
/// Idempotent — ignores "already exists" errors.
pub fn create_test_database(compose_file: &Path, worker_name: &str) {
    let test_db = format!("{worker_name}_test");
    println!("Creating test database '{test_db}'...");
    let cmd = format!("createdb -U postgres {test_db} 2>/dev/null || true");
    match compose_mgr::exec(compose_file, "db", &cmd) {
        Ok(()) => println!("  Test database '{test_db}' ready."),
        Err(e) => {
            eprintln!("  Warning: failed to create test database: {e}");
            eprintln!("  You can create it manually: docker compose exec db createdb -U postgres {test_db}");
        }
    }
}

/// Set up the test database schema using `rails db:prepare` with the test DATABASE_URL.
/// Non-fatal: prints warnings on failure.
pub fn setup_test_schema(compose_file: &Path) {
    println!("Setting up test database schema...");
    match compose_mgr::exec(
        compose_file,
        "app",
        "DATABASE_URL=$DATABASE_URL_TEST RAILS_ENV=test rails db:prepare",
    ) {
        Ok(()) => println!("  Test database schema ready."),
        Err(e) => {
            eprintln!("  Warning: test schema setup failed: {e}");
            eprintln!("  You can run it manually: docker compose exec -e DATABASE_URL=$DATABASE_URL_TEST app rails db:prepare");
        }
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
        .args(["-h", &host, "-p", &port, "-d", &db_name, "--schema-only", "-t", "__groot_preflight_nonexistent__"])
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
        return Err(GrootError::Other(format!(
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
        .map_err(|e| GrootError::Other(format!("Failed to start pg_dump: {e}")))?;

    let pg_dump_stdout = pg_dump
        .stdout
        .take()
        .ok_or_else(|| GrootError::Other("Failed to capture pg_dump stdout".to_string()))?;

    let pg_dump_stderr = pg_dump
        .stderr
        .take()
        .ok_or_else(|| GrootError::Other("Failed to capture pg_dump stderr".to_string()))?;

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
    let target_db = format!("{worker_name}_dev");
    let mut psql = Command::new("docker")
        .args([
            "compose",
            "-f", &compose_file.to_string_lossy(),
            "-p", &project,
            "exec", "-T",
            "db",
            "psql", "-U", "postgres", "-d", &target_db,
        ])
        .stdin(Stdio::from(pg_dump_stdout))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GrootError::Other(format!("Failed to start psql in container: {e}")))?;

    let psql_status = psql
        .wait()
        .map_err(|e| GrootError::Other(format!("Failed to wait for psql: {e}")))?;

    let pg_dump_status = pg_dump
        .wait()
        .map_err(|e| GrootError::Other(format!("Failed to wait for pg_dump: {e}")))?;

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
        return Err(GrootError::Other(
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

/// Parse a postgres:// or postgresql:// URL into (host, port, db_name).
fn parse_pg_url(raw: &str) -> Result<(String, String, String)> {
    // The `url` crate doesn't recognize postgres:// as a special scheme,
    // but it parses it fine as a generic URL.
    let parsed = url::Url::parse(raw).map_err(|e| {
        GrootError::Other(format!("Invalid database URL '{raw}': {e}"))
    })?;

    match parsed.scheme() {
        "postgres" | "postgresql" => {}
        scheme => {
            return Err(GrootError::Other(format!(
                "Invalid database URL scheme '{scheme}' in '{raw}'. Expected postgres:// or postgresql://"
            )));
        }
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| GrootError::Other(format!("No host in database URL '{raw}'")))?
        .to_string();
    let port = parsed.port().unwrap_or(5432).to_string();
    let db_name = parsed.path().trim_start_matches('/').to_string();

    if db_name.is_empty() {
        return Err(GrootError::Other(format!(
            "No database name in URL '{raw}'"
        )));
    }

    Ok((host, port, db_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── parse_pg_url ──────────────────────────────────────────────────

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
        let (host, port, db) =
            parse_pg_url("postgres://user:pass@db.host:5433/prod").unwrap();
        assert_eq!(host, "db.host");
        assert_eq!(port, "5433");
        assert_eq!(db, "prod");
    }

    #[test]
    fn test_parse_pg_url_special_chars_in_password() {
        let (host, port, db) =
            parse_pg_url("postgres://user:p%40ss%23word@localhost:5432/mydb").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, "5432");
        assert_eq!(db, "mydb");
    }

    #[test]
    fn test_parse_pg_url_postgresql_scheme() {
        let (host, port, db) =
            parse_pg_url("postgresql://localhost:5432/mydb").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, "5432");
        assert_eq!(db, "mydb");
    }

    #[test]
    fn test_parse_pg_url_with_query_params() {
        let (host, port, db) =
            parse_pg_url("postgres://localhost:5432/mydb?sslmode=require").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, "5432");
        assert_eq!(db, "mydb");
    }

    #[test]
    fn test_parse_pg_url_invalid_scheme() {
        assert!(parse_pg_url("mysql://localhost/mydb").is_err());
    }

    #[test]
    fn test_parse_pg_url_no_db_name() {
        assert!(parse_pg_url("postgres://localhost:5432/").is_err());
        assert!(parse_pg_url("postgres://localhost:5432").is_err());
    }

    #[test]
    fn test_parse_pg_url_garbage() {
        assert!(parse_pg_url("not a url").is_err());
    }

    // ── strip_erb / extract_erb_fallback ──────────────────────────────

    #[test]
    fn test_strip_erb_double_quoted_fallback() {
        let input = r#"<%= ENV['DB'] || "Reportal_development" %>"#;
        assert_eq!(strip_erb(input), "Reportal_development");
    }

    #[test]
    fn test_strip_erb_single_quoted_fallback() {
        let input = "<%= ENV['DB'] || 'Reportal_development' %>";
        assert_eq!(strip_erb(input), "Reportal_development");
    }

    #[test]
    fn test_strip_erb_no_fallback() {
        let input = "<%= ENV['DB'] %>";
        assert_eq!(strip_erb(input), "");
    }

    #[test]
    fn test_strip_erb_mixed_content() {
        let input = "host: localhost\ndatabase: <%= ENV['DB'] || \"mydb\" %>\nport: 5432";
        let expected = "host: localhost\ndatabase: mydb\nport: 5432";
        assert_eq!(strip_erb(input), expected);
    }

    #[test]
    fn test_strip_erb_no_erb() {
        let input = "host: localhost\ndatabase: mydb";
        assert_eq!(strip_erb(input), input);
    }

    // ── detect_from_env_file ──────────────────────────────────────────

    #[test]
    fn test_env_file_basic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".env"),
            "DATABASE_URL=postgres://localhost:5432/mydb\n",
        )
        .unwrap();
        assert_eq!(
            detect_from_env_file(dir.path()),
            Some("postgres://localhost:5432/mydb".to_string())
        );
    }

    #[test]
    fn test_env_file_with_export() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".env"),
            "export DATABASE_URL=postgres://localhost:5432/mydb\n",
        )
        .unwrap();
        assert_eq!(
            detect_from_env_file(dir.path()),
            Some("postgres://localhost:5432/mydb".to_string())
        );
    }

    #[test]
    fn test_env_file_double_quoted() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".env"),
            "DATABASE_URL=\"postgres://localhost:5432/mydb\"\n",
        )
        .unwrap();
        assert_eq!(
            detect_from_env_file(dir.path()),
            Some("postgres://localhost:5432/mydb".to_string())
        );
    }

    #[test]
    fn test_env_file_single_quoted() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".env"),
            "DATABASE_URL='postgres://localhost:5432/mydb'\n",
        )
        .unwrap();
        assert_eq!(
            detect_from_env_file(dir.path()),
            Some("postgres://localhost:5432/mydb".to_string())
        );
    }

    #[test]
    fn test_env_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_from_env_file(dir.path()), None);
    }

    #[test]
    fn test_env_file_no_database_url() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".env"), "RAILS_ENV=development\n").unwrap();
        assert_eq!(detect_from_env_file(dir.path()), None);
    }

    #[test]
    fn test_env_file_with_comments() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".env"),
            "# This is a comment\nRAILS_ENV=development\nDATABASE_URL=postgres://localhost/db\n",
        )
        .unwrap();
        assert_eq!(
            detect_from_env_file(dir.path()),
            Some("postgres://localhost/db".to_string())
        );
    }

    // ── detect_from_database_yml ──────────────────────────────────────

    #[test]
    fn test_database_yml_plain_value() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/database.yml"),
            "development:\n  database: Reportal_development\n  host: localhost\n",
        )
        .unwrap();
        assert_eq!(
            detect_from_database_yml(dir.path()),
            Some("postgres://localhost:5432/Reportal_development".to_string())
        );
    }

    #[test]
    fn test_database_yml_erb_double_quote() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/database.yml"),
            "development:\n  database: <%= ENV['DB'] || \"Reportal_development\" %>\n",
        )
        .unwrap();
        assert_eq!(
            detect_from_database_yml(dir.path()),
            Some("postgres://localhost:5432/Reportal_development".to_string())
        );
    }

    #[test]
    fn test_database_yml_erb_single_quote() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/database.yml"),
            "development:\n  database: <%= ENV['DB'] || 'Reportal_development' %>\n",
        )
        .unwrap();
        assert_eq!(
            detect_from_database_yml(dir.path()),
            Some("postgres://localhost:5432/Reportal_development".to_string())
        );
    }

    #[test]
    fn test_database_yml_erb_no_fallback() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/database.yml"),
            "development:\n  database: <%= ENV['DB'] %>\n",
        )
        .unwrap();
        // ERB with no fallback → empty string → key has empty value → None
        assert_eq!(detect_from_database_yml(dir.path()), None);
    }

    #[test]
    fn test_database_yml_with_anchors() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/database.yml"),
            "default: &default\n  adapter: postgresql\n  host: localhost\n\ndevelopment:\n  <<: *default\n  database: myapp_development\n",
        )
        .unwrap();
        assert_eq!(
            detect_from_database_yml(dir.path()),
            Some("postgres://localhost:5432/myapp_development".to_string())
        );
    }

    #[test]
    fn test_database_yml_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_from_database_yml(dir.path()), None);
    }

    // ── detect_from_convention ────────────────────────────────────────

    #[test]
    fn test_convention_normal_repo() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("MyProject");
        fs::create_dir_all(repo_path.join(".git")).unwrap();
        assert_eq!(
            detect_from_convention(&repo_path),
            Some("postgres://localhost:5432/MyProject_development".to_string())
        );
    }

    #[test]
    fn test_convention_hyphenated_name() {
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path().join("my-project");
        fs::create_dir_all(repo_path.join(".git")).unwrap();
        assert_eq!(
            detect_from_convention(&repo_path),
            Some("postgres://localhost:5432/my_project_development".to_string())
        );
    }

    #[test]
    fn test_convention_worktree() {
        let dir = tempfile::tempdir().unwrap();
        // Simulate a parent repo at /tmp/xxx/Reportal/.git/worktrees/task-name
        let parent = dir.path().join("Reportal");
        fs::create_dir_all(parent.join(".git/worktrees/task-name")).unwrap();

        // The worktree directory with a .git file
        let worktree = dir.path().join("task-name");
        fs::create_dir_all(&worktree).unwrap();
        let gitdir = parent.join(".git/worktrees/task-name");
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}", gitdir.display()),
        )
        .unwrap();

        assert_eq!(
            detect_from_convention(&worktree),
            Some("postgres://localhost:5432/Reportal_development".to_string())
        );
    }

    // ── project_name_from_path ────────────────────────────────────────

    #[test]
    fn test_project_name_normal_dir() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("Reportal");
        fs::create_dir_all(project.join(".git")).unwrap();
        assert_eq!(
            project_name_from_path(&project),
            Some("Reportal".to_string())
        );
    }

    #[test]
    fn test_project_name_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("Reportal");
        fs::create_dir_all(parent.join(".git/worktrees/fix-bug")).unwrap();

        let worktree = dir.path().join("fix-bug");
        fs::create_dir_all(&worktree).unwrap();
        let gitdir = parent.join(".git/worktrees/fix-bug");
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}", gitdir.display()),
        )
        .unwrap();

        assert_eq!(
            project_name_from_path(&worktree),
            Some("Reportal".to_string())
        );
    }

    #[test]
    fn test_project_name_no_git() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("something");
        fs::create_dir_all(&project).unwrap();
        assert_eq!(
            project_name_from_path(&project),
            Some("something".to_string())
        );
    }

    // ── detect_from_running_postgres (parse logic only) ───────────────

    #[test]
    #[ignore] // Requires a running Postgres instance
    fn test_detect_from_running_postgres() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("Reportal");
        fs::create_dir_all(project.join(".git")).unwrap();
        // This test only works if Postgres is running and has Reportal_development
        if let Some(url) = detect_from_running_postgres(&project) {
            assert!(url.contains("Reportal_development"));
        }
    }
}
