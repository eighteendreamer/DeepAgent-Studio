//! Apply the product's schema migrations to an existing, backed-up database.
use deepagent_persistence::migrations;
use rusqlite::Connection;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: migrate_schema <existing-database>")?;
    if !std::path::Path::new(&path).is_file() {
        return Err("database must already exist".into());
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    let before = migrations::current_version(&conn)?;
    migrations::run(&conn)?;
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err("database integrity check failed".into());
    }
    println!(
        "schema: {before} -> {}; integrity: ok",
        migrations::current_version(&conn)?
    );
    Ok(())
}
