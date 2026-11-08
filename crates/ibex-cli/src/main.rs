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
    /// SMS and iMessage conversations.
    #[command(subcommand)]
    Sms(SmsCommand),
    /// WhatsApp chats.
    #[command(subcommand)]
    Wa(WaCommand),
    /// The camera roll.
    #[command(subcommand)]
    Photos(PhotosCommand),
    /// Voice Memos.
    #[command(subcommand)]
    Voice(VoiceCommand),
    /// Files: On My iPhone and per-app Documents.
    #[command(subcommand)]
    Docs(DocsCommand),
    /// The address book.
    #[command(subcommand)]
    Contacts(ContactsCommand),
    /// Call history.
    #[command(subcommand)]
    Calls(CallsCommand),
    /// Export conversations to a folder.
    Export(ExportArgs),
    /// Build the full-text search index over SMS + WhatsApp.
    Index(IndexArgs),
    /// Search the index built by `index`.
    Search(SearchArgs),
    /// Write a synthetic backup, for exercising the CLI without a real one.
    #[cfg(feature = "fixture")]
    Fixture(FixtureArgs),
}

/// The backup folder, plus the password option that milestone M7 will use.
#[derive(Args)]
struct BackupArgs {
    /// Path to the backup folder: the one containing Manifest.db.
    backup: PathBuf,
    /// Prompt for the backup password (encrypted backups).
    ///
    /// The password is read from the `IBEX_PASSWORD` environment variable if
    /// set, otherwise from the terminal. It is deliberately not accepted as an
    /// argument, which would put it in shell history and in `ps` output.
    #[arg(long)]
    password: bool,
}

impl BackupArgs {
    fn open(&self) -> Result<Backup> {
        let password = if self.password {
            Some(read_password()?)
        } else {
            None
        };
        match Backup::open(&self.backup, password.as_deref()) {
            Ok(backup) => Ok(backup),
            Err(e) if e.code() == "password_required" => {
                // Retry with a prompt rather than making the user guess the
                // flag they needed.
                let password = read_password()?;
                Backup::open(&self.backup, Some(&password))
                    .with_context(|| format!("opening {}", self.backup.display()))
            }
            Err(e) => Err(anyhow::Error::new(e))
                .with_context(|| format!("opening {}", self.backup.display())),
        }
    }
}

/// Read the backup password from the environment or the terminal.
///
/// Never echoed, never logged, never taken from argv.
fn read_password() -> Result<String> {
    if let Ok(password) = std::env::var("IBEX_PASSWORD")
        && !password.is_empty()
    {
        return Ok(password);
    }
    eprint!("Backup password: ");
    std::io::stderr().flush()?;
    let mut password = String::new();
    std::io::stdin()
        .read_line(&mut password)
        .context("reading the password")?;
    eprintln!();
    // Terminals leave the newline on; nothing else is trimmed, since a
    // password may legitimately contain spaces.
    Ok(password.trim_end_matches(['\n', '\r']).to_string())
}

#[derive(Subcommand)]
enum SmsCommand {
    /// List conversations.
    Convs(SmsConvsArgs),
    /// List messages in one conversation.
    Msgs(SmsMsgsArgs),
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum CliFormat {
    Html,
    Csv,
    Json,
    Files,
}

impl From<CliFormat> for ibex_core::ExportFormat {
    fn from(value: CliFormat) -> Self {
        match value {
            CliFormat::Html => ibex_core::ExportFormat::Html,
            CliFormat::Csv => ibex_core::ExportFormat::Csv,
            CliFormat::Json => ibex_core::ExportFormat::Json,
            CliFormat::Files => ibex_core::ExportFormat::Files,
        }
    }
}

#[derive(Args)]
struct ExportArgs {
    #[command(flatten)]
    backup: BackupArgs,
    /// Which source: sms, wa or contacts.
    source: String,
    /// A conversation id, or `all`.
    conversation: String,
    #[arg(long, value_enum, default_value_t = CliFormat::Html)]
    format: CliFormat,
    /// Directory to write into. Created if missing.
    #[arg(long)]
    out: PathBuf,
    /// Do not copy attachment files.
    #[arg(long)]
    no_attachments: bool,
    /// Include WhatsApp status and channel feeds.
    #[arg(long)]
    all_sessions: bool,
}

#[derive(Args)]
struct IndexArgs {
    #[command(flatten)]
    backup: BackupArgs,
}

#[derive(Args)]
struct SearchArgs {
    #[command(flatten)]
    backup: BackupArgs,
    /// The search text.
    query: String,
    /// Restrict to one source: sms or whatsapp.
    #[arg(long)]
    source: Option<String>,
    #[arg(long, default_value_t = 20)]
    limit: u32,
    /// Show matched snippets. Off by default: these are personal.
    #[arg(long)]
    show_content: bool,
}

#[derive(Subcommand)]
enum WaCommand {
    /// List chats.
    Chats(WaChatsArgs),
    /// List messages in one chat.
    Msgs(WaMsgsArgs),
}

#[derive(Args)]
struct WaChatsArgs {
    #[command(flatten)]
    backup: BackupArgs,
    #[arg(long, default_value_t = 50)]
    limit: u32,
    /// Also list status, channel and broadcast feeds, which are hidden by
    /// default because they are not conversations.
    #[arg(long)]
    all_sessions: bool,
    /// Show names and previews. Off by default: these are personal.
    #[arg(long)]
    show_content: bool,
}

#[derive(Args)]
struct WaMsgsArgs {
    #[command(flatten)]
    backup: BackupArgs,
    /// Chat id, as printed by `wa chats`.
    chat: String,
    #[arg(long, default_value_t = 20)]
    limit: u32,
    /// Show message text and sender names. Off by default.
    #[arg(long)]
    show_content: bool,
}

#[derive(Subcommand)]
enum PhotosCommand {
    /// List assets, newest first.
    List(PhotosListArgs),
    /// Per-month counts, as the grid's headers use.
    Timeline(PhotosFilterArgs),
    /// Titled albums with their asset counts.
    Albums(PhotosFilterArgs),
    /// Resolve every asset against the backup and report the totals.
    ///
    /// This is the check that matters: an asset row with no file behind it is
    /// a blank tile in the grid.
    Audit(PhotosFilterArgs),
}

#[derive(Args)]
struct PhotosFilterArgs {
    #[command(flatten)]
    backup: BackupArgs,
    /// Only `photo` or only `video`.
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    favorites: bool,
    #[arg(long)]
    screenshots: bool,
    /// The Recently Deleted album.
    #[arg(long)]
    trashed: bool,
    /// The Hidden album.
    #[arg(long)]
    hidden: bool,
    /// Restrict to one album, by the id `photos albums` prints.
    #[arg(long)]
    album: Option<String>,
}

#[derive(Args)]
struct PhotosListArgs {
    #[command(flatten)]
    filter: PhotosFilterArgs,
    #[arg(long, default_value_t = 20)]
    limit: u32,
    /// Show file names. Off by default: a file name can be personal.
    #[arg(long)]
    show_content: bool,
}

#[derive(Subcommand)]
enum VoiceCommand {
    /// List recordings, newest first.
    List(VoiceListArgs),
}

#[derive(Args)]
struct VoiceListArgs {
    #[command(flatten)]
    backup: BackupArgs,
    #[arg(long, default_value_t = 50)]
    limit: u32,
    /// Show titles. Off by default: a recording's title is personal.
    #[arg(long)]
    show_content: bool,
}

#[derive(Subcommand)]
enum DocsCommand {
    /// List the browsable Files sources: On My iPhone and per-app Documents.
    Sources(DocsSourcesArgs),
}

#[derive(Args)]
struct DocsSourcesArgs {
    #[command(flatten)]
    backup: BackupArgs,
}

#[derive(Subcommand)]
enum ContactsCommand {
    /// List every contact.
    List(ContactsListArgs),
}

#[derive(Args)]
struct ContactsListArgs {
    #[command(flatten)]
    backup: BackupArgs,
    #[arg(long, default_value_t = 200)]
    limit: u32,
    /// Show names, numbers and emails. Off by default: these are personal.
    #[arg(long)]
    show_content: bool,
}

#[derive(Subcommand)]
enum CallsCommand {
    /// List call history, newest first.
    List(CallsListArgs),
}

#[derive(Args)]
struct CallsListArgs {
    #[command(flatten)]
    backup: BackupArgs,
    #[arg(long, default_value_t = 50)]
    limit: u32,
    /// Show resolved names and numbers. Off by default: these are personal.
    #[arg(long)]
    show_content: bool,
}

#[derive(Args)]
struct SmsConvsArgs {
    #[command(flatten)]
    backup: BackupArgs,
    #[arg(long, default_value_t = 50)]
    limit: u32,
    /// Show names and message previews. Off by default: these are personal.
    #[arg(long)]
    show_content: bool,
}

#[derive(Args)]
struct SmsMsgsArgs {
    #[command(flatten)]
    backup: BackupArgs,
    /// Conversation id, as printed by `sms convs`.
    conversation: String,
    #[arg(long, default_value_t = 20)]
    limit: u32,
    /// Show message text and sender names. Off by default.
    #[arg(long)]
    show_content: bool,
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
        Command::Sms(SmsCommand::Convs(args)) => sms_convs(args),
        Command::Sms(SmsCommand::Msgs(args)) => sms_msgs(args),
        Command::Wa(WaCommand::Chats(args)) => wa_chats(args),
        Command::Wa(WaCommand::Msgs(args)) => wa_msgs(args),
        Command::Photos(PhotosCommand::List(args)) => photos_list(args),
        Command::Photos(PhotosCommand::Timeline(args)) => photos_timeline(args),
        Command::Photos(PhotosCommand::Albums(args)) => photos_albums(args),
        Command::Photos(PhotosCommand::Audit(args)) => photos_audit(args),
        Command::Voice(VoiceCommand::List(args)) => voice_list(args),
        Command::Docs(DocsCommand::Sources(args)) => docs_sources(args),
        Command::Contacts(ContactsCommand::List(args)) => contacts_list(args),
        Command::Calls(CallsCommand::List(args)) => calls_list(args),
        Command::Export(args) => export(args),
        Command::Index(args) => index(args),
        Command::Search(args) => search_cmd(args),
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
/// Export conversations.
fn export(args: &ExportArgs) -> Result<()> {
    use ibex_core::export::{CancelToken, Progress};
    use ibex_core::{ExportOptions, Selection};

    // `wa` on the command line, `whatsapp` internally.
    let source = match args.source.as_str() {
        "sms" => "sms",
        "wa" | "whatsapp" => "whatsapp",
        "contacts" => "contacts",
        other => bail!("unknown source {other:?}; expected sms, wa or contacts"),
    };
    let selection = if args.conversation == "all" {
        Selection::all(source)
    } else {
        Selection::one(source, &args.conversation)
    };
    let options = ExportOptions {
        include_attachments: !args.no_attachments,
        include_broadcasts: args.all_sessions,
        ..Default::default()
    };

    let backup = args.backup.open()?;
    let format: ibex_core::ExportFormat = args.format.into();

    // Progress to stderr, so stdout stays the report. The trait takes `Fn`,
    // so the last-seen counter lives in a Cell rather than being captured
    // mutably.
    let last = std::cell::Cell::new(0u64);
    let progress = |p: Progress| {
        if p.done >= last.get() {
            eprint!("\r{} ({}/{})", p.message, p.done, p.total.unwrap_or(0));
            let _ = std::io::stderr().flush();
            last.set(p.done);
        }
    };

    let report = ibex_core::export::run(
        &backup,
        &selection,
        format,
        &args.out,
        &options,
        &progress,
        &CancelToken::new(),
    )?;
    eprintln!();

    println!("Exported to    {}", report.output_dir);
    println!("Format         {}", report.format);
    println!("Source         {}", report.source);
    println!("Conversations  {}", report.totals.conversations);
    println!("Messages       {}", report.totals.messages);
    println!("Attachments    {} copied", report.totals.attachments_copied);
    if report.totals.attachments_missing > 0 {
        println!(
            "               {} referenced but not in the backup",
            report.totals.attachments_missing
        );
    }
    println!("Bytes written  {}", human_size(report.totals.bytes_written));
    if report.formulas_neutralised > 0 {
        println!(
            "CSV formulas   {} cell(s) prefixed so a spreadsheet shows them as text",
            report.formulas_neutralised
        );
    }
    if report.cancelled {
        println!("Status         CANCELLED - this export is incomplete");
    }
    for warning in &report.warnings {
        println!("Warning        {warning}");
    }
    println!("\nReport         {}/export-report.json", report.output_dir);
    Ok(())
}

/// Build the full-text search index over SMS + WhatsApp.
fn index(args: &IndexArgs) -> Result<()> {
    use ibex_core::export::{CancelToken, Progress};

    let backup = args.backup.open()?;

    let last = std::cell::Cell::new(0u64);
    let progress = |p: Progress| {
        if p.done >= last.get() {
            eprint!("\r{} ({}/{})", p.message, p.done, p.total.unwrap_or(0));
            let _ = std::io::stderr().flush();
            last.set(p.done);
        }
    };

    let report = ibex_core::search::build_index(&backup, &progress, &CancelToken::new())?;
    eprintln!();

    println!("Conversations indexed  {}", report.conversations_indexed);
    println!("Messages indexed       {}", report.messages_indexed);
    if report.cancelled {
        println!("Status                 CANCELLED - this index is incomplete");
    }
    Ok(())
}

/// Search the index built by `index`. Ids and dates by default, snippet
/// text only on request.
fn search_cmd(args: &SearchArgs) -> Result<()> {
    use ibex_core::search::SearchFilters;
    use ibex_core::{Page, search};

    let backup = args.backup.open()?;
    let status = search::index_status(&backup)?;
    if !status.built {
        bail!("no search index yet; run `ibex-cli index` first");
    }

    let filters = SearchFilters {
        source: args.source.clone(),
        conversation_id: None,
    };
    let hits = search::search(&backup, &args.query, &filters, Page::first(args.limit))?;

    println!(
        "{:<8} {:<9} {:<20}  {}",
        "SOURCE",
        "CONV",
        "SENT",
        if args.show_content {
            "SNIPPET"
        } else {
            "MESSAGE ID"
        }
    );
    for hit in &hits.items {
        let sent = hit
            .sent_at
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "-".to_string());
        let tail = if args.show_content {
            hit.snippet.replace(['\u{1}', '\u{2}'], "*")
        } else {
            hit.message_id.clone()
        };
        println!(
            "{:<8} {:<9} {:<20}  {}",
            hit.source, hit.conversation_id, sent, tail
        );
    }
    println!(
        "\n{} of {} match(es)",
        hits.items.len(),
        hits.total.unwrap_or(0)
    );
    if !args.show_content {
        println!("(snippets withheld; pass --show-content to print them)");
    }
    Ok(())
}

/// List WhatsApp chats.
fn wa_chats(args: &WaChatsArgs) -> Result<()> {
    use ibex_core::extractors::whatsapp::WhatsAppDb;

    let backup = args.backup.open()?;
    let db = WhatsAppDb::open(&backup).context("opening ChatStorage.sqlite")?;
    let page = db.list_conversations(Page::first(args.limit), args.all_sessions)?;

    println!("container: {}", db.domain());
    println!(
        "{:<8} {:<7} {:>8}  {:<20}  {}",
        "ID",
        "KIND",
        "MSGS",
        "LAST ACTIVITY",
        if args.show_content { "NAME" } else { "" }
    );
    for conv in &page.items {
        let kind = if conv.is_group { "group" } else { "1:1" };
        let last = conv
            .last_message_at
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".to_string());
        if args.show_content {
            println!(
                "{:<8} {:<7} {:>8}  {:<20}  {}",
                conv.id,
                kind,
                conv.message_count,
                last,
                conv.label()
            );
        } else {
            println!(
                "{:<8} {:<7} {:>8}  {:<20}  <{} participant(s)>",
                conv.id,
                kind,
                conv.message_count,
                last,
                conv.participants.len()
            );
        }
    }
    println!(
        "\n{} of {} chats{}",
        page.items.len(),
        page.total.unwrap_or(0),
        if args.all_sessions {
            " (including status and channels)"
        } else {
            " (status and channels hidden; --all-sessions to include)"
        }
    );
    if !args.show_content {
        println!("(names and previews withheld; pass --show-content to print them)");
    }
    Ok(())
}

/// List messages in one WhatsApp chat.
fn wa_msgs(args: &WaMsgsArgs) -> Result<()> {
    use ibex_core::MessageCursor;
    use ibex_core::extractors::whatsapp::WhatsAppDb;

    let backup = args.backup.open()?;
    let db = WhatsAppDb::open(&backup).context("opening ChatStorage.sqlite")?;
    let page = db.list_messages(&args.chat, MessageCursor::first(args.limit))?;

    println!(
        "{:<20}  {:<4} {:<9} {:<4}  {}",
        "SENT",
        "DIR",
        "KIND",
        "ATT",
        if args.show_content { "TEXT" } else { "ID" }
    );
    for message in &page.messages {
        let sent = message
            .sent_at
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "-".to_string());
        let kind = format!("{:?}", message.kind);
        let tail = if args.show_content {
            message
                .body
                .clone()
                .unwrap_or_else(|| "<no text>".to_string())
        } else {
            message.id.clone()
        };
        println!(
            "{:<20}  {:<4} {:<9} {:<4}  {}",
            sent,
            if message.from_me { "out" } else { "in" },
            &kind[..kind.len().min(9)],
            message.attachments.len(),
            tail
        );
    }
    println!("\n{} message(s)", page.messages.len());
    if page.older.is_some() {
        println!("(older messages remain; raise --limit to see more)");
    }
    if !args.show_content {
        println!("(message text withheld; pass --show-content to print it)");
    }
    Ok(())
}

/// List conversations. Counts and ids by default, content only on request.
fn sms_convs(args: &SmsConvsArgs) -> Result<()> {
    use ibex_core::extractors::sms::SmsDb;

    let backup = args.backup.open()?;
    let db = SmsDb::open(&backup).context("opening sms.db")?;
    let page = db.list_conversations(Page::first(args.limit))?;

    println!(
        "{:<8} {:<7} {:>8}  {:<20}  {}",
        "ID",
        "KIND",
        "MSGS",
        "LAST ACTIVITY",
        if args.show_content { "NAME" } else { "" }
    );
    for conv in &page.items {
        let kind = if conv.is_group { "group" } else { "1:1" };
        let last = conv
            .last_message_at
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".to_string());
        if args.show_content {
            println!(
                "{:<8} {:<7} {:>8}  {:<20}  {}",
                conv.id,
                kind,
                conv.message_count,
                last,
                conv.label()
            );
        } else {
            println!(
                "{:<8} {:<7} {:>8}  {:<20}  <{} participant(s)>",
                conv.id,
                kind,
                conv.message_count,
                last,
                conv.participants.len()
            );
        }
    }
    println!(
        "\n{} of {} conversations",
        page.items.len(),
        page.total.unwrap_or(0)
    );
    if !args.show_content {
        println!("(names and previews withheld; pass --show-content to print them)");
    }
    Ok(())
}

/// List messages in one conversation.
fn sms_msgs(args: &SmsMsgsArgs) -> Result<()> {
    use ibex_core::MessageCursor;
    use ibex_core::extractors::sms::SmsDb;

    let backup = args.backup.open()?;
    let db = SmsDb::open(&backup).context("opening sms.db")?;
    let page = db.list_messages(&args.conversation, MessageCursor::first(args.limit))?;

    println!(
        "{:<20}  {:<4} {:<9} {:<4} {:<4}  {}",
        "SENT",
        "DIR",
        "KIND",
        "ATT",
        "RX",
        if args.show_content { "TEXT" } else { "ID" }
    );
    for message in &page.messages {
        let sent = message
            .sent_at
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "-".to_string());
        let kind = format!("{:?}", message.kind);
        let tail = if args.show_content {
            message
                .body
                .clone()
                .unwrap_or_else(|| "<no text>".to_string())
        } else {
            message.id.clone()
        };
        println!(
            "{:<20}  {:<4} {:<9} {:<4} {:<4}  {}",
            sent,
            if message.from_me { "out" } else { "in" },
            &kind[..kind.len().min(9)],
            message.attachments.len(),
            message.reactions.len(),
            tail
        );
    }
    println!("\n{} message(s)", page.messages.len());
    if page.older.is_some() {
        println!("(older messages remain; raise --limit to see more)");
    }
    if !args.show_content {
        println!("(message text withheld; pass --show-content to print it)");
    }
    Ok(())
}

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
        .with_sample_media()
        .with_file(
            "HomeDomain",
            "Library/Preferences/com.apple.nope.plist",
            b"x",
        )
        .with_file("HomeDomain", "Library/SMS/Drafts/draft.plist", b"draft")
        .with_file(
            "MediaDomain",
            "Library/SMS/Attachments/ab/01/IMG_0001.HEIC",
            b"heic",
        )
        .with_file(
            "CameraRollDomain",
            "Media/DCIM/100APPLE/IMG_0001.JPG",
            b"jpeg",
        )
        .with_phantom_file(
            "MediaDomain",
            "Library/SMS/Attachments/cd/02/gone.jpg",
            98_765,
        )
        .with_sms()
        .with_whatsapp()
        .with_address_book()
        .with_call_history()
        .build(&args.out)
        .context("building the fixture")?;

    println!("{}", root.display());
    eprintln!(
        "Synthetic backup written. Try:\n  \
         ibex-cli info {root}\n  \
         ibex-cli domains {root}\n  \
         ibex-cli ls {root} HomeDomain Library/SMS\n  \
         ibex-cli sql {root} HomeDomain Library/SMS/sms.db\n  \
         ibex-cli sms convs {root}\n  \
         ibex-cli sms msgs {root} 1\n  \
         ibex-cli wa chats {root}\n  \
         ibex-cli wa msgs {root} 1\n  \
         ibex-cli contacts list {root}\n  \
         ibex-cli calls list {root}",
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

/// Translate the CLI's filter flags into the extractor's filter.
fn photo_filter(args: &PhotosFilterArgs) -> Result<ibex_core::extractors::PhotoFilter> {
    use ibex_core::extractors::{PhotoFilter, PhotoKind};

    let kind = match args.kind.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None => None,
        Some("photo") | Some("photos") | Some("image") => Some(PhotoKind::Photo),
        Some("video") | Some("videos") => Some(PhotoKind::Video),
        Some(other) => anyhow::bail!("--kind must be `photo` or `video`, not {other:?}"),
    };
    Ok(PhotoFilter {
        kind,
        favorites_only: args.favorites,
        screenshots_only: args.screenshots,
        trashed_only: args.trashed,
        hidden_only: args.hidden,
        album_id: args.album.clone(),
    })
}

fn open_photos(
    args: &PhotosFilterArgs,
) -> Result<(ibex_core::Backup, ibex_core::extractors::PhotoFilter)> {
    let backup = args.backup.open()?;
    let filter = photo_filter(args)?;
    Ok((backup, filter))
}

fn photos_list(args: &PhotosListArgs) -> Result<()> {
    use ibex_core::extractors::{PhotoCursor, PhotoKind, PhotosDb};

    let (backup, filter) = open_photos(&args.filter)?;
    let db = PhotosDb::open(&backup).context("opening Photos.sqlite")?;
    let total = db.count(&filter)?;
    let page = db.list(&filter, PhotoCursor::first(args.limit))?;

    println!("asset table: {}", db.asset_table());
    println!(
        "{:<8} {:<5} {:<17} {:<9} {:<6} {:>10}  {}",
        "ID",
        "KIND",
        "TAKEN",
        "PIXELS",
        "FLAGS",
        "BYTES",
        if args.show_content { "FILE" } else { "FILES" }
    );
    for asset in &page.assets {
        let taken = asset
            .created_at
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".to_string());
        let kind = match asset.kind {
            PhotoKind::Photo => "img",
            PhotoKind::Video => "vid",
        };
        // One letter per flag, so a glance shows the shape of the library.
        let flags: String = [
            (asset.live, 'L'),
            (asset.edited, 'E'),
            (asset.favorite, 'F'),
            (asset.screenshot, 'S'),
            (asset.trashed, 'T'),
            (asset.hidden, 'H'),
        ]
        .iter()
        .map(|(set, ch)| if *set { *ch } else { '.' })
        .collect();
        let tail = if args.show_content {
            asset.file_name.clone()
        } else {
            // Which files resolved, without naming them.
            format!(
                "{}{}{}{}",
                if asset.file_id.is_some() {
                    "original "
                } else {
                    ""
                },
                if asset.thumb_file_id.is_some() {
                    "thumb "
                } else {
                    ""
                },
                if asset.live_file_id.is_some() {
                    "live "
                } else {
                    ""
                },
                if asset.edited_file_id.is_some() {
                    "render"
                } else {
                    ""
                },
            )
        };
        println!(
            "{:<8} {:<5} {:<17} {:<9} {:<6} {:>10}  {}",
            asset.id,
            kind,
            taken,
            format!("{}x{}", asset.width, asset.height),
            flags,
            asset.size.unwrap_or(0),
            tail.trim_end(),
        );
    }
    println!("\n{} of {total} assets", page.assets.len());
    println!("flags: L live  E edited  F favourite  S screenshot  T trashed  H hidden");
    if !args.show_content {
        println!("(file names withheld; pass --show-content to print them)");
    }
    Ok(())
}

fn photos_timeline(args: &PhotosFilterArgs) -> Result<()> {
    use ibex_core::extractors::PhotosDb;

    let (backup, filter) = open_photos(args)?;
    let db = PhotosDb::open(&backup).context("opening Photos.sqlite")?;
    let months = db.timeline(&filter)?;

    let widest = months.iter().map(|m| m.count).max().unwrap_or(1).max(1);
    for month in &months {
        // A bar, so the shape of the library is visible at a glance.
        let width = (month.count * 40 / widest).max(1) as usize;
        println!(
            "{:04}-{:02}  {:>6}  {}",
            month.year,
            month.month,
            month.count,
            "#".repeat(width)
        );
    }
    let total: u64 = months.iter().map(|m| m.count).sum();
    println!("\n{} months, {total} assets", months.len());
    let counted = db.count(&filter)?;
    if total != counted {
        println!("note: {} assets have no capture date", counted - total);
    }
    Ok(())
}

fn photos_albums(args: &PhotosFilterArgs) -> Result<()> {
    use ibex_core::extractors::PhotosDb;

    let (backup, _) = open_photos(args)?;
    let db = PhotosDb::open(&backup).context("opening Photos.sqlite")?;
    let albums = db.albums()?;

    println!("join table: {}", db.album_join_table().unwrap_or("<none>"));
    println!("{:<8} {:>8}  TITLE", "ID", "ASSETS");
    for album in &albums {
        println!("{:<8} {:>8}  {}", album.id, album.count, album.title);
    }
    println!("\n{} titled album(s)", albums.len());
    Ok(())
}

fn photos_audit(args: &PhotosFilterArgs) -> Result<()> {
    use ibex_core::extractors::{PhotoAvailability, PhotoCursor, PhotosDb};

    let (backup, filter) = open_photos(args)?;
    let db = PhotosDb::open(&backup).context("opening Photos.sqlite")?;

    let mut totals = AuditTotals::default();
    let mut cursor = Some(PhotoCursor::first(PhotoCursor::MAX_LIMIT));
    while let Some(c) = cursor {
        let page = db.list(&filter, c)?;
        for asset in &page.assets {
            totals.assets += 1;
            if asset.file_id.is_some() {
                totals.originals += 1;
            }
            if asset.thumb_file_id.is_some() {
                totals.thumbs += 1;
            }
            if asset.live {
                totals.live += 1;
                if asset.live_file_id.is_some() {
                    totals.live_resolved += 1;
                }
            }
            if asset.edited {
                totals.edited += 1;
                if asset.edited_file_id.is_some() {
                    totals.edited_resolved += 1;
                }
            }
            match asset.availability {
                PhotoAvailability::InBackup => {}
                PhotoAvailability::CloudOnly => totals.cloud_only += 1,
                PhotoAvailability::Missing => totals.missing += 1,
            }
        }
        cursor = page.older;
    }

    let percent = |n: u64| {
        if totals.assets == 0 {
            0.0
        } else {
            n as f64 * 100.0 / totals.assets as f64
        }
    };
    println!("assets                {:>8}", totals.assets);
    println!(
        "originals resolved    {:>8}  {:.1}%",
        totals.originals,
        percent(totals.originals)
    );
    println!(
        "derivatives resolved  {:>8}  {:.1}%",
        totals.thumbs,
        percent(totals.thumbs)
    );
    println!(
        "live photos           {:>8}  companions resolved {}",
        totals.live, totals.live_resolved
    );
    println!(
        "edited                {:>8}  renders resolved    {}",
        totals.edited, totals.edited_resolved
    );
    println!("cloud-only            {:>8}", totals.cloud_only);
    println!("missing               {:>8}", totals.missing);
    Ok(())
}

#[derive(Default)]
struct AuditTotals {
    assets: u64,
    originals: u64,
    thumbs: u64,
    live: u64,
    live_resolved: u64,
    edited: u64,
    edited_resolved: u64,
    cloud_only: u64,
    missing: u64,
}

fn voice_list(args: &VoiceListArgs) -> Result<()> {
    use ibex_core::Page;
    use ibex_core::extractors::{RecordingAvailability, VoiceMemosDb};

    let backup = args.backup.open()?;
    let db = VoiceMemosDb::open(&backup).context("opening the Voice Memos library")?;
    let page = db.list(Page::first(args.limit))?;

    println!(
        "source: {}",
        if db.is_database_backed() {
            "CloudRecordings.db"
        } else {
            "file listing (database missing or unreadable)"
        }
    );
    println!(
        "{:<8} {:<17} {:>8}  {:>10}  {}",
        "ID",
        "DATE",
        "DURATION",
        "BYTES",
        if args.show_content { "TITLE" } else { "STATUS" }
    );
    for recording in &page.items {
        let date = recording
            .created_at
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".to_string());
        let duration = recording
            .duration
            .map(|d| format!("{:.0}s", d))
            .unwrap_or_else(|| "-".to_string());
        let tail = if args.show_content {
            recording
                .title
                .clone()
                .unwrap_or_else(|| "(untitled)".to_string())
        } else {
            match recording.availability {
                RecordingAvailability::InBackup => "in backup".to_string(),
                RecordingAvailability::Missing => "missing".to_string(),
            }
        };
        println!(
            "{:<8} {:<17} {:>8}  {:>10}  {}",
            recording.id,
            date,
            duration,
            recording.size.unwrap_or(0),
            tail,
        );
    }
    println!(
        "\n{} of {} recordings",
        page.items.len(),
        page.total.unwrap_or(0)
    );
    if !args.show_content {
        println!("(titles withheld; pass --show-content to print them)");
    }
    Ok(())
}

fn docs_sources(args: &DocsSourcesArgs) -> Result<()> {
    use ibex_core::extractors::list_document_sources;

    let backup = args.backup.open()?;
    let sources = list_document_sources(&backup).context("listing Files sources")?;

    println!("{:<60} {:<20} LABEL", "DOMAIN", "BUNDLE ID");
    for source in &sources {
        println!(
            "{:<60} {:<20} {}",
            source.domain,
            source.bundle_id.as_deref().unwrap_or("-"),
            source.label,
        );
    }
    println!("\n{} source(s)", sources.len());
    Ok(())
}

fn contacts_list(args: &ContactsListArgs) -> Result<()> {
    use ibex_core::Page;
    use ibex_core::extractors::ContactsDb;

    let backup = args.backup.open()?;
    let db = ContactsDb::open(&backup).context("opening AddressBook.sqlitedb")?;
    let page = db.list(Page::first(args.limit))?;

    println!(
        "{:<8} {:<7} {:<7}  {}",
        "ID",
        "PHONES",
        "EMAILS",
        if args.show_content { "NAME" } else { "" }
    );
    for contact in &page.items {
        if args.show_content {
            println!(
                "{:<8} {:<7} {:<7}  {}",
                contact.id,
                contact.phones.len(),
                contact.emails.len(),
                contact.display_name.as_deref().unwrap_or("(unnamed)"),
            );
        } else {
            println!(
                "{:<8} {:<7} {:<7}",
                contact.id,
                contact.phones.len(),
                contact.emails.len(),
            );
        }
    }
    println!(
        "\n{} of {} contact(s)",
        page.items.len(),
        page.total.unwrap_or(0)
    );
    if !args.show_content {
        println!("(names withheld; pass --show-content to print them)");
    }
    Ok(())
}

fn calls_list(args: &CallsListArgs) -> Result<()> {
    use ibex_core::Page;
    use ibex_core::extractors::{CallHistoryDb, CallService};

    let backup = args.backup.open()?;
    let db = CallHistoryDb::open(&backup).context("opening CallHistory.storedata")?;
    let page = db.list(Page::first(args.limit))?;

    println!(
        "{:<20}  {:<4} {:<10} {:<8}  {}",
        "DATE",
        "DIR",
        "SERVICE",
        "ANSWERED",
        if args.show_content { "FROM" } else { "ID" }
    );
    for call in &page.items {
        let date = call
            .date
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "-".to_string());
        let service = match &call.service {
            CallService::Telephony => "cellular".to_string(),
            CallService::FaceTime => "facetime".to_string(),
            CallService::Other(s) if s.is_empty() => "unknown".to_string(),
            CallService::Other(s) => s.clone(),
        };
        let answered = match call.answered {
            Some(true) => "yes",
            Some(false) => "no",
            None => "-",
        };
        let tail = if args.show_content {
            call.contact.label().to_string()
        } else {
            call.id.clone()
        };
        println!(
            "{:<20}  {:<4} {:<10} {:<8}  {}",
            date,
            if call.outgoing { "out" } else { "in" },
            service,
            answered,
            tail,
        );
    }
    println!(
        "\n{} of {} call(s)",
        page.items.len(),
        page.total.unwrap_or(0)
    );
    if !args.show_content {
        println!("(names/numbers withheld; pass --show-content to print them)");
    }
    Ok(())
}
