//! `ibex-cli`: a thin command-line tool over `ibex-core`, for debugging
//! backups and for verifying every extractor before it is wired into the GUI.
//!
//! Output is deliberately structural — counts, IDs, paths and sizes. Device
//! identifiers are masked unless `--full` is passed, so output can be pasted
//! into a bug report. Nothing here prints message text or contact details.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use ibex_core::{Backup, EntryKind, ManifestEntry, Page};

#[derive(Parser)]
#[command(
    name = "ibex-cli",
    about = "Inspect an iPhone backup from the command line",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Device and backup metadata.
    Info(InfoArgs),
    /// Every domain, with file counts.
    Domains(DomainsArgs),
    /// List the contents of a directory within a domain.
    Ls(LsArgs),
    /// Write a file's bytes to stdout.
    Cat(CatArgs),
    /// Run a read-only query against a database in the backup.
    Sql(SqlArgs),
    /// Write a synthetic backup, for exercising the CLI without a real one.
    #[cfg(feature = "fixture")]
    Fixture(FixtureArgs),
}

/// The backup folder, plus the password option that milestone M7 will use.
#[derive(Args)]
struct BackupArgs {
    /// Path to the backup folder: the one containing Manifest.db.
    backup: PathBuf,
    /// Password for an encrypted backup (milestone M7).
    #[arg(long)]
    password: Option<String>,
}

impl BackupArgs {
    fn open(&self) -> Result<Backup> {
        Backup::open(&self.backup, self.password.as_deref())
            .with_context(|| format!("opening {}", self.backup.display()))
    }
}

#[derive(Args)]
struct InfoArgs {
    #[command(flatten)]
    backup: BackupArgs,
    /// Show the UDID and serial number in full instead of masked.
    #[arg(long)]
    full: bool,
    /// Also total up file sizes per domain (reads every metadata blob).
    #[arg(long)]
    sizes: bool,
}

#[derive(Args)]
struct DomainsArgs {
    #[command(flatten)]
    backup: BackupArgs,
    /// Total up file sizes per domain (reads every metadata blob; slow).
    #[arg(long)]
    sizes: bool,
}

#[derive(Args)]
struct LsArgs {
    #[command(flatten)]
    backup: BackupArgs,
    /// Domain to list, e.g. HomeDomain.
    domain: String,
    /// Directory within the domain. Defaults to its root.
    #[arg(default_value = "")]
    path: String,
    /// Maximum entries to show.
    #[arg(long, default_value_t = 200)]
    limit: u32,
    /// Entries to skip.
    #[arg(long, default_value_t = 0)]
    offset: u64,
    /// Show file IDs.
    #[arg(long)]
    ids: bool,
    /// List every entry, paging until the end.
    #[arg(long)]
    all: bool,
}

#[derive(Args)]
struct CatArgs {
    #[command(flatten)]
    backup: BackupArgs,
    domain: String,
    path: String,
}

#[derive(Args)]
struct SqlArgs {
    #[command(flatten)]
    backup: BackupArgs,
    domain: String,
    /// Path to the database within the domain, e.g. Library/SMS/sms.db.
    path: String,
    /// A single SELECT or PRAGMA statement. Without one, prints the schema and
    /// per-table row counts.
    query: Option<String>,
    /// Print result rows. Off by default: rows can contain personal data.
    #[arg(long)]
    rows: bool,
    /// Maximum rows to print with --rows.
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[cfg(feature = "fixture")]
#[derive(Args)]
struct FixtureArgs {
    /// Directory to write the synthetic backup into.
    #[arg(long)]
    out: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Info(args) => info(args),
        Command::Domains(args) => domains(args),
        Command::Ls(args) => ls(args),
        Command::Cat(args) => cat(args),
        Command::Sql(args) => sql(args),
        #[cfg(feature = "fixture")]
        Command::Fixture(args) => fixture(args),
    }
}

/// Hide the middle of an identifying string: `ABCD…1234`.
fn mask(value: &str, show: bool) -> String {
    if show || value.len() <= 8 {
        return value.to_string();
    }
    format!("{}…{}", &value[..4], &value[value.len() - 4..])
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn info(args: &InfoArgs) -> Result<()> {
    let backup = args.backup.open()?;
    let info = backup.info();
    let unknown = "unknown".to_string();

    println!("Backup      {}", backup.root().display());
    println!("Cache id    {}", backup.id());
    println!(
        "Device      {}",
        info.device_name.clone().unwrap_or_else(|| unknown.clone())
    );
    println!(
        "Model       {} ({})",
        info.product_name.clone().unwrap_or_else(|| unknown.clone()),
        info.product_type.clone().unwrap_or_else(|| unknown.clone())
    );
    println!(
        "iOS         {} ({})",
        info.product_version
            .clone()
            .unwrap_or_else(|| unknown.clone()),
        info.build_version
            .clone()
            .unwrap_or_else(|| unknown.clone())
    );
    println!(
        "UDID        {}",
        mask(info.udid.as_deref().unwrap_or("unknown"), args.full)
    );
    println!(
        "Serial      {}",
        mask(
            info.serial_number.as_deref().unwrap_or("unknown"),
            args.full
        )
    );
    println!(
        "Backup date {}",
        info.date()
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| unknown.clone())
    );
    println!("Encrypted   {}", info.encrypted);
    println!(
        "Full backup {}",
        info.is_full_backup
            .map(|b| b.to_string())
            .unwrap_or_else(|| unknown.clone())
    );
    println!("Apps        {}", info.installed_apps.len());

    let domains = if args.sizes {
        summarize_with_progress(&backup)?
    } else {
        backup.domains()?
    };
    let files = backup.manifest().count_files()?;
    println!("Domains     {}", domains.len());
    println!("Entries     {files}");
    if args.sizes {
        let total: u64 = domains.iter().filter_map(|d| d.total_size).sum();
        println!("Total size  {}", human_size(total));
    }
    println!("Cache size  {}", human_size(backup.cache().size_on_disk()?));
    if !args.full {
        println!("\n(UDID and serial are masked; pass --full to show them)");
    }
    Ok(())
}

fn domains(args: &DomainsArgs) -> Result<()> {
    let backup = args.backup.open()?;
    let domains = if args.sizes {
        summarize_with_progress(&backup)?
    } else {
        backup.domains()?
    };

    let width = domains
        .iter()
        .map(|d| d.domain.len())
        .max()
        .unwrap_or(0)
        .min(60);
    for domain in &domains {
        match domain.total_size {
            Some(size) => println!(
                "{:<width$}  {:>8}  {:>10}",
                domain.domain,
                domain.file_count,
                human_size(size)
            ),
            None => println!("{:<width$}  {:>8}", domain.domain, domain.file_count),
        }
    }
    println!("\n{} domains", domains.len());
    Ok(())
}

/// Run the size scan, reporting progress to stderr so stdout stays parseable.
fn summarize_with_progress(backup: &Backup) -> Result<Vec<ibex_core::DomainSummary>> {
    let mut last = 0;
    let domains = backup.domains_with_sizes(&mut |done, total| {
        if done >= last + 20_000 || done == total {
            eprint!("\rscanning metadata: {done}/{total}");
            let _ = std::io::stderr().flush();
            last = done;
        }
    })?;
    eprintln!();
    Ok(domains)
}

fn ls(args: &LsArgs) -> Result<()> {
    let backup = args.backup.open()?;

    let mut shown = 0u64;
    let mut offset = args.offset;
    // The total is the same on every page; keep the first one we are given.
    let mut total: Option<u64> = None;
    loop {
        let page = backup.list_dir(&args.domain, &args.path, Page::new(offset, args.limit))?;
        total = total.or(page.total);
        for entry in &page.items {
            print_entry(entry, args.ids);
            shown += 1;
        }
        match page.next_offset {
            Some(next) if args.all => offset = next,
            _ => break,
        }
    }

    match total {
        Some(total) if total > shown => {
            println!("\n{shown} of {total} entries (--all for the rest)")
        }
        Some(total) => println!("\n{total} entries"),
        None => println!("\n{shown} entries"),
    }
    Ok(())
}

fn print_entry(entry: &ManifestEntry, show_ids: bool) {
    let kind = match entry.kind {
        EntryKind::File => "f",
        EntryKind::Directory => "d",
        EntryKind::Symlink => "l",
        EntryKind::Other(_) => "?",
    };
    let size = if entry.kind.is_file() {
        human_size(entry.size())
    } else {
        "-".to_string()
    };
    let mtime = entry
        .meta
        .as_ref()
        .and_then(|m| m.mtime)
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".to_string());

    if show_ids {
        println!(
            "{kind} {:>10}  {mtime:>16}  {}  {}",
            size, entry.file_id, entry.relative_path
        );
    } else {
        println!("{kind} {:>10}  {mtime:>16}  {}", size, entry.relative_path);
    }
}

fn cat(args: &CatArgs) -> Result<()> {
    let backup = args.backup.open()?;
    let entry = backup
        .find(&args.domain, &args.path)?
        .with_context(|| format!("no entry {}:{}", args.domain, args.path))?;

    if entry.kind.is_dir() {
        bail!("{}:{} is a directory", args.domain, args.path);
    }

    // Straight to stdout, so binary files survive a redirect intact.
    let mut reader = backup.open_file(&entry)?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    std::io::copy(&mut reader, &mut out).context("writing to stdout")?;
    out.flush()?;
    Ok(())
}

fn sql(args: &SqlArgs) -> Result<()> {
    let backup = args.backup.open()?;
    let conn = backup
        .open_db(&args.domain, &args.path)
        .with_context(|| format!("opening {}:{}", args.domain, args.path))?;

    let Some(query) = &args.query else {
        return print_schema(&conn);
    };

    let query = query.trim().trim_end_matches(';');
    guard_read_only(query)?;

    let mut stmt = conn.prepare(query).context("preparing the query")?;
    // The authoritative check: SQLite itself says whether the statement can
    // write. This catches what a keyword test cannot, such as
    // "WITH x AS (...) DELETE FROM ...".
    if !stmt.readonly() {
        bail!("refusing to run a statement that can modify the database");
    }

    let columns: Vec<String> = stmt.column_names().map(str::to_string).collect();
    let column_count = columns.len();

    if !args.rows {
        // Count the result rows without printing any of their values.
        let mut rows = stmt.query([])?;
        let mut n = 0usize;
        while rows.next()?.is_some() {
            n += 1;
        }
        println!("{} column(s): {}", column_count, columns.join(", "));
        println!("{n} row(s)");
        println!("\n(values withheld; pass --rows to print them)");
        return Ok(());
    }

    println!("{}", columns.join(" | "));
    let mut rows = stmt.query([])?;
    let mut n = 0usize;
    while let Some(row) = rows.next()? {
        if n >= args.limit {
            println!("… (stopped at --limit {})", args.limit);
            break;
        }
        let cells: Vec<String> = (0..column_count).map(|i| format_cell(row, i)).collect();
        println!("{}", cells.join(" | "));
        n += 1;
    }
    Ok(())
}

/// Reject anything that is not obviously a read.
///
/// Belt and braces: the connection is opened against a working copy, this
/// keyword check runs first, and `Statement::readonly` is checked after
/// preparing.
fn guard_read_only(query: &str) -> Result<()> {
    if query.is_empty() {
        bail!("empty query");
    }
    // One statement only: a trailing semicolon has already been trimmed, so
    // any remaining one separates statements.
    if query.contains(';') {
        bail!("only a single statement is allowed");
    }
    let first = query
        .split(|c: char| c.is_whitespace() || c == '(')
        .find(|w| !w.is_empty())
        .unwrap_or_default()
        .to_ascii_uppercase();
    if !matches!(first.as_str(), "SELECT" | "PRAGMA" | "WITH" | "EXPLAIN") {
        bail!("only SELECT, PRAGMA, WITH and EXPLAIN statements are allowed, got {first}");
    }
    Ok(())
}

fn format_cell(row: &rusqlite::Row<'_>, index: usize) -> String {
    use rusqlite::types::ValueRef;
    match row.get_ref(index) {
        Ok(ValueRef::Null) => "NULL".to_string(),
        Ok(ValueRef::Integer(i)) => i.to_string(),
        Ok(ValueRef::Real(f)) => f.to_string(),
        Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).to_string(),
        Ok(ValueRef::Blob(b)) => format!("<{} bytes>", b.len()),
        Err(e) => format!("<error: {e}>"),
    }
}

/// Print the schema and per-table row counts. No values.
fn print_schema(conn: &rusqlite::Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT name, type FROM sqlite_master \
         WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )?;
    let objects: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;

    if objects.is_empty() {
        println!("no tables");
        return Ok(());
    }

    for (name, kind) in &objects {
        let count: i64 = conn
            // The name comes from sqlite_master, so it is a real identifier;
            // quote it anyway in case it needs it.
            .query_one(&format!("SELECT count(*) FROM \"{name}\""), [], |r| {
                r.get(0)
            })
            .unwrap_or(-1);
        println!("{kind} {name}  ({count} rows)");

        let mut columns = conn.prepare("SELECT name, type FROM pragma_table_info(?1)")?;
        let cols: Vec<String> = columns
            .query_map([name], |row| {
                let col: String = row.get(0)?;
                let ty: String = row.get(1)?;
                Ok(if ty.is_empty() {
                    col
                } else {
                    format!("{col} {ty}")
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        for col in cols {
            println!("    {col}");
        }
    }
    println!("\n{} tables and views", objects.len());
    Ok(())
}

#[cfg(feature = "fixture")]
fn fixture(args: &FixtureArgs) -> Result<()> {
    use ibex_core::testutil::FixtureBackup;

    let root = FixtureBackup::new()
        .with_device_name("Fixture iPhone")
        .with_installed_app("net.whatsapp.WhatsApp")
        .with_file("HomeDomain", "Library/Preferences/com.apple.nope.plist", b"x")
        .with_file("HomeDomain", "Library/SMS/Drafts/draft.plist", b"draft")
        .with_file("MediaDomain", "Library/SMS/Attachments/ab/01/IMG_0001.HEIC", b"heic")
        .with_file("CameraRollDomain", "Media/DCIM/100APPLE/IMG_0001.JPG", b"jpeg")
        .with_phantom_file("MediaDomain", "Library/SMS/Attachments/cd/02/gone.jpg", 98_765)
        .with_wal_db(
            "HomeDomain",
            "Library/SMS/sms.db",
            "CREATE TABLE message (ROWID INTEGER PRIMARY KEY, text TEXT, date INTEGER, is_from_me INTEGER);
             CREATE TABLE chat (ROWID INTEGER PRIMARY KEY, chat_identifier TEXT, display_name TEXT);
             CREATE TABLE attachment (ROWID INTEGER PRIMARY KEY, filename TEXT, mime_type TEXT);
             INSERT INTO chat (chat_identifier, display_name) VALUES ('fixture-1', 'Fixture chat');
             INSERT INTO message (text, date, is_from_me) VALUES ('synthetic', 0, 0);",
            "INSERT INTO message (text, date, is_from_me) VALUES ('in the wal', 0, 1);",
        )
        .build(&args.out)
        .context("building the fixture")?;

    println!("{}", root.display());
    eprintln!(
        "Synthetic backup written. Try:\n  \
         ibex-cli info {root}\n  \
         ibex-cli domains {root}\n  \
         ibex-cli ls {root} HomeDomain Library/SMS\n  \
         ibex-cli sql {root} HomeDomain Library/SMS/sms.db",
        root = root.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking_hides_the_middle() {
        assert_eq!(mask("0123456789abcdef", false), "0123…cdef");
        assert_eq!(mask("0123456789abcdef", true), "0123456789abcdef");
        // Too short to mask usefully: shown as-is rather than mangled.
        assert_eq!(mask("short", false), "short");
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1024 * 1024 * 3 / 2), "1.5 MiB");
    }

    #[test]
    fn the_sql_guard_allows_reads() {
        for query in [
            "SELECT count(*) FROM message",
            "select 1",
            "  PRAGMA table_info('message')",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "EXPLAIN SELECT 1",
            "(SELECT 1)",
        ] {
            guard_read_only(query).unwrap_or_else(|e| panic!("{query:?} should pass: {e}"));
        }
    }

    #[test]
    fn the_sql_guard_refuses_writes_and_stacked_statements() {
        for query in [
            "DELETE FROM message",
            "UPDATE message SET text = 'x'",
            "INSERT INTO message (text) VALUES ('x')",
            "DROP TABLE message",
            "ATTACH DATABASE '/tmp/x' AS x",
            "VACUUM",
            "CREATE TABLE t (a)",
            "SELECT 1; DELETE FROM message",
            "",
        ] {
            assert!(
                guard_read_only(query).is_err(),
                "{query:?} should be refused"
            );
        }
    }

    /// `WITH ... DELETE` passes the keyword check, so the `Statement::readonly`
    /// check behind it is what actually stops it. This test pins that down.
    #[test]
    fn sqlite_itself_catches_a_write_hidden_behind_with() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (a INTEGER);  INSERT INTO t VALUES (1);")
            .unwrap();

        let sneaky = "WITH x AS (SELECT 1) DELETE FROM t";
        assert!(
            guard_read_only(sneaky).is_ok(),
            "the keyword check alone lets this through"
        );
        let stmt = conn.prepare(sneaky).unwrap();
        assert!(
            !stmt.readonly(),
            "Statement::readonly must flag it as a write"
        );

        let honest = conn.prepare("SELECT * FROM t").unwrap();
        assert!(honest.readonly());
    }
}
