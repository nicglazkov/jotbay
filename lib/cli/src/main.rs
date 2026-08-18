mod dash;
mod render;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use jotbay_core::{Jotbay, VERSION};

#[derive(Parser)]
#[command(
    name = "jotbay",
    version = VERSION,
    about = "Keep your markdown notes in sync across machines",
    long_about = "Keeps your markdown notes in sync across machines through a private git remote.\n\
                  Run `jotbay dash` for a live dashboard."
)]
struct Cli {
    /// Jotbay location. Defaults to the enclosing repo, then ~/jotbay.
    #[arg(long, global = true, value_name = "PATH")]
    jotbay: Option<PathBuf>,

    /// Emit machine-readable JSON instead of formatted output.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show local and network-wide sync state (default)
    Status {
        /// Skip the network fetch and report only what is already local
        #[arg(long)]
        offline: bool,
    },
    /// Commit, integrate the remote, and push
    Sync,
    /// List every machine that has reported in
    Nodes {
        /// Remove a decommissioned machine's status
        #[arg(long, value_name = "HOSTNAME")]
        forget: Option<String>,
    },
    /// What happened to your notes
    Activity {
        #[arg(short = 'n', long, default_value_t = 25)]
        limit: usize,
        /// Skip the network fetch
        #[arg(long)]
        offline: bool,
        /// Show what each machine did, rather than what changed
        #[arg(long)]
        raw: bool,
    },
    /// Commit history: what changed
    Log {
        #[arg(short = 'n', long, default_value_t = 15)]
        limit: u32,
    },
    /// Deal with an interrupted rebase
    Resolve {
        /// Discard the in-progress rebase and return to a clean tree
        #[arg(long)]
        abort: bool,
    },
    /// Live dashboard
    Dash,
    /// Print the path of the synced data directory
    Path,
    /// Set up a vault on this machine for the first time
    Init {
        /// Create a private GitHub repository and clone it
        #[arg(long, value_name = "NAME")]
        create: Option<String>,
        /// Clone a repository you already have
        #[arg(long, value_name = "URL")]
        clone: Option<String>,
        /// Adopt a directory that is already a clone
        #[arg(long, value_name = "PATH")]
        adopt: Option<PathBuf>,
        /// Where to put it (default ~/jotbay)
        #[arg(long, value_name = "PATH")]
        at: Option<PathBuf>,
    },
    /// Put a shortcut to the app or to your notes folder on the desktop
    Shortcut {
        /// app or notes; both if omitted
        #[arg(value_name = "WHAT")]
        what: Option<String>,
        /// Where to put it (default: your desktop)
        #[arg(long, value_name = "PATH")]
        at: Option<PathBuf>,
    },
    /// Show or set up the background sync this machine runs
    Schedule,
    /// Sync automatically whenever the notes change, until stopped
    ///
    /// What the background schedule runs. Also useful by hand to watch it work.
    Watch,
    /// Fetch the current release and replace this machine's binaries
    Upgrade,
    /// Find a note by name or by what is in it
    Find {
        query: Vec<String>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Every version of one note
    History {
        file: String,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: u32,
    },
    /// Notes that were deleted and can be brought back
    Deleted {
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: u32,
    },
    /// Put a note, or an old version of one, back
    Restore {
        file: String,
        /// The version to restore. Omit for a deleted note's last version.
        version: Option<String>,
    },
    /// Start a note
    New {
        name: String,
        /// Text to put in it. Otherwise it is created empty.
        text: Vec<String>,
    },
    /// Add a line to a note, creating it if needed
    Add {
        file: String,
        text: Vec<String>,
    },
    /// Open a note in whichever editor this machine uses
    Edit { file: String },
    /// Conflict copies waiting to be settled, or settle one
    Conflicts {
        /// The conflict copy to settle
        copy: Option<String>,
        /// keep-current, keep-copy, or keep-both
        #[arg(long, value_name = "CHOICE")]
        settle: Option<String>,
    },
    /// Show this machine: version, notes, remote, and background sync
    About,
    /// Show or change per-machine preferences
    Settings {
        /// theme=system|light|dark, verbose=on|off
        #[arg(value_name = "KEY=VALUE")]
        assignment: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Handled before discovery: the whole point is that there is no vault yet.
    if let Some(Command::Init { create, clone, adopt, at }) = &cli.command {
        return match cmd_init(cli.json, create.clone(), clone.clone(), adopt.clone(), at.clone()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                render::error(&e.to_string());
                ExitCode::FAILURE
            }
        };
    }

    let jotbay = match Jotbay::discover(cli.jotbay.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            render::error(&e.to_string());
            return ExitCode::FAILURE;
        }
    };

    let result = match cli.command.unwrap_or(Command::Status { offline: false }) {
        Command::Status { offline } => cmd_status(&jotbay, cli.json, !offline),
        Command::Sync => cmd_sync(&jotbay, cli.json),
        Command::Nodes { forget } => cmd_nodes(&jotbay, cli.json, forget),
        Command::Activity { limit, offline, raw } => cmd_activity(&jotbay, cli.json, limit, !offline, raw),
        Command::Log { limit } => cmd_log(&jotbay, cli.json, limit),
        Command::Resolve { abort } => cmd_resolve(&jotbay, abort),
        Command::Dash => dash::run(&jotbay).map_err(|e| jotbay_core::Error::Other(e.to_string())),
        Command::Path => {
            println!("{}", jotbay.data_dir().display());
            Ok(())
        }
        Command::Init { .. } => unreachable!("handled before discovery"),
        Command::Schedule => cmd_schedule(),
        Command::Watch => cmd_watch(&jotbay),
        Command::Shortcut { what, at } => cmd_shortcut(&jotbay, what, at),
        Command::Upgrade => cmd_upgrade(&jotbay, cli.json),
        Command::Find { query, limit } => cmd_find(&jotbay, cli.json, &query.join(" "), limit),
        Command::History { file, limit } => cmd_history(&jotbay, cli.json, &file, limit),
        Command::Deleted { limit } => cmd_deleted(&jotbay, cli.json, limit),
        Command::Restore { file, version } => cmd_restore(&jotbay, &file, version),
        Command::New { name, text } => cmd_new(&jotbay, &name, &text.join(" ")),
        Command::Add { file, text } => cmd_add(&jotbay, &file, &text.join(" ")),
        Command::Edit { file } => cmd_edit(&jotbay, &file),
        Command::Conflicts { copy, settle } => cmd_conflicts(&jotbay, cli.json, copy, settle),
        Command::About => cmd_about(&jotbay, cli.json),
        Command::Settings { assignment } => cmd_settings(cli.json, assignment),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Still a non-zero exit: the sync genuinely did not happen, and a
            // script that pushes and then checks deserves to know. Only the
            // wording softens, because there is no fault to report.
            if jotbay_core::git::looks_offline(&e.to_string()) {
                render::offline_notice();
            } else {
                render::error(&e.to_string());
            }
            ExitCode::FAILURE
        }
    }
}

/// Everything a settings panel shows, for people who do not open one.
///
/// Deliberately offline. Opening settings should not be a request to GitHub,
/// so the update line reports the marker the repository already carries.
fn cmd_find(jotbay: &Jotbay, json: bool, query: &str, limit: usize) -> jotbay_core::Result<()> {
    let hits = jotbay.search(query, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
    } else {
        render::hits(&hits, query);
    }
    Ok(())
}

fn cmd_history(jotbay: &Jotbay, json: bool, file: &str, limit: u32) -> jotbay_core::Result<()> {
    let versions = jotbay.history(file, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&versions)?);
    } else {
        render::history(&versions, file);
    }
    Ok(())
}

fn cmd_deleted(jotbay: &Jotbay, json: bool, limit: u32) -> jotbay_core::Result<()> {
    let gone = jotbay.deleted_notes(limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&gone)?);
    } else {
        render::deleted(&gone);
    }
    Ok(())
}

/// Restore an old version, or a note that is gone entirely.
///
/// One command for both, because from the outside they are the same request.
/// Which one it is can be worked out rather than asked: a note that is not
/// there is being undeleted.
fn cmd_restore(jotbay: &Jotbay, file: &str, version: Option<String>) -> jotbay_core::Result<()> {
    let written = match version {
        Some(sha) => jotbay.restore_version(file, &sha)?,
        None => {
            let gone = jotbay.deleted_notes(200)?;
            let entry = gone.iter().find(|d| d.path == file).ok_or_else(|| {
                jotbay_core::Error::Other(format!(
                    "{file} is not a deleted note. Give a version from `jotbay history {file}`."
                ))
            })?;
            jotbay.restore_deleted(file, &entry.sha)?
        }
    };
    render::wrote(&written, "restored");
    Ok(())
}

fn cmd_new(jotbay: &Jotbay, name: &str, text: &str) -> jotbay_core::Result<()> {
    let written = jotbay.create_note(name, text)?;
    render::wrote(&written, "created");
    Ok(())
}

fn cmd_add(jotbay: &Jotbay, file: &str, text: &str) -> jotbay_core::Result<()> {
    if text.trim().is_empty() {
        return Err(jotbay_core::Error::Other("nothing to add".into()));
    }
    let written = jotbay.append_note(file, text)?;
    render::wrote(&written, "added to");
    Ok(())
}

fn cmd_edit(jotbay: &Jotbay, file: &str) -> jotbay_core::Result<()> {
    jotbay.open_note_externally(file)?;
    Ok(())
}

fn cmd_conflicts(
    jotbay: &Jotbay,
    json: bool,
    copy: Option<String>,
    settle: Option<String>,
) -> jotbay_core::Result<()> {
    use jotbay_core::pairs::Settle;

    if let Some(copy) = copy {
        let choice = match settle.as_deref() {
            Some("keep-current") => Settle::KeepCurrent,
            Some("keep-copy") => Settle::KeepCopy,
            Some("keep-both") => Settle::KeepBoth,
            Some(other) => {
                return Err(jotbay_core::Error::Other(format!(
                    "'{other}' is not a way to settle. Use keep-current, keep-copy or keep-both."
                )))
            }
            None => {
                return Err(jotbay_core::Error::Other(
                    "say how: --settle keep-current, keep-copy or keep-both".into(),
                ))
            }
        };
        jotbay.settle_conflict(&copy, choice)?;
        render::settled();
        return Ok(());
    }

    let pairs = jotbay.conflict_pairs()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&pairs)?);
    } else {
        render::conflict_pairs(&pairs);
    }
    Ok(())
}

fn cmd_about(jotbay: &Jotbay, json: bool) -> jotbay_core::Result<()> {
    let about = jotbay.about()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&about)?);
    } else {
        render::about(&about);
    }
    Ok(())
}

fn cmd_status(jotbay: &Jotbay, json: bool, refresh: bool) -> jotbay_core::Result<()> {
    let status = jotbay.status(refresh)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        render::status(&status);
    }
    Ok(())
}

fn cmd_sync(jotbay: &Jotbay, json: bool) -> jotbay_core::Result<()> {
    if !json {
        println!();
    }
    // A machine set up here should sync by itself afterwards. Doing it only in
    // the installer meant anyone who arrived through the .dmg or the .msi got
    // a tool that synced when asked and at no other time.
    match jotbay_core::schedule::ensure() {
        Ok(true) => println!("  background sync set up"),
        Ok(false) => {}
        Err(e) => render::error(&format!("could not set up background sync: {e}")),
    }

    let report = jotbay.sync()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render::sync_report(&report);
        println!();
    }

    // A sync that never ran must not exit 0. The watcher holds the lock most of
    // the time, so `jotbay sync` frequently did nothing at all and still
    // reported success, and a script checking the exit code could not tell
    // "synced" from "did not sync". Found during the Windows fresh install,
    // where a runbook step depended on this command actually pushing.
    //
    // 75 rather than 1: distinguishable from a real failure, and the
    // conventional "temporary failure, try again" code.
    if report.skipped_locked {
        std::process::exit(75);
    }
    Ok(())
}

fn cmd_nodes(jotbay: &Jotbay, json: bool, forget: Option<String>) -> jotbay_core::Result<()> {
    if let Some(host) = forget {
        jotbay.forget_node(&host)?;
        println!("  forgot {host}");
        return Ok(());
    }

    // Ask before reading. This is the command whose entire purpose is "who is
    // out there", and without it the answer is only ever "who last had
    // something to sync", which on a quiet fleet is nobody at all.
    // Best-effort: a remote we cannot reach still has local answers worth
    // showing.
    let asked = jotbay_core::presence::request(jotbay.git()).is_ok();
    let nodes = jotbay.nodes(true)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&nodes)?);
    } else {
        let head = jotbay.git().head().unwrap_or_default();
        render::nodes(&nodes, &head);
        // Answers cannot arrive before they are asked for. A machine that has
        // been quiet checks the remote every few minutes, so the roll call sent
        // a moment ago is still in flight while this table is being printed
        // and the first run after a quiet spell therefore shows ages, not
        // presence. Saying so beats letting the reader conclude their machines
        // are dead.
        let waiting = nodes
            .iter()
            .filter(|n| n.is_stale(jotbay_core::SYNC_INTERVAL_SECS))
            .count();
        if asked && waiting > 0 {
            println!();
            println!(
                "  asked {} to report in. Run this again in a moment.",
                if waiting == 1 { "a machine that has been quiet".to_string() }
                else { format!("{waiting} machines that have been quiet") }
            );
        }
        println!();
    }
    Ok(())
}

fn cmd_init(
    json: bool,
    create: Option<String>,
    clone: Option<String>,
    adopt: Option<PathBuf>,
    at: Option<PathBuf>,
) -> jotbay_core::Result<()> {
    use jotbay_core::setup;

    // With no action given, report what this machine can do. A first-run screen
    // asks the same question, so both surfaces read the same answer.
    if create.is_none() && clone.is_none() && adopt.is_none() {
        let caps = setup::capabilities();
        if json {
            println!("{}", serde_json::to_string_pretty(&caps)?);
        } else {
            render::capabilities(&caps);
        }
        return Ok(());
    }

    let destination = at.unwrap_or_else(jotbay_core::default_root);

    let root = if let Some(name) = create {
        println!("  Creating a private repository and cloning it.");
        setup::create_and_clone(&name, &destination)?
    } else if let Some(url) = clone {
        println!("  Cloning.");
        setup::clone_existing(&url, &destination)?
    } else {
        setup::adopt(&adopt.expect("one of the three is set"))?
    };

    let jotbay = Jotbay::open(&root)?;
    // Record it before syncing. A GUI opened afterwards finds the vault by this
    // setting, and it should still do so even if this first sync fails.
    jotbay.remember()?;

    // A machine set up here must sync by itself afterwards. This lived only in
    // `cmd_sync`, so setting up through `init`. The route the install scripts
    // tell people to run, produced a vault with no background sync at all,
    // and nothing said so. Verified absent on a fresh macOS VM: `init`
    // reported success and `~/Library/LaunchAgents` did not exist. The same
    // shape as issue #3 on Windows, on a third path.
    match jotbay_core::schedule::ensure() {
        Ok(true) => println!("  background sync set up"),
        Ok(false) => {}
        Err(e) => render::error(&format!("could not set up background sync: {e}")),
    }

    let report = jotbay.sync()?;
    println!();
    render::sync_report(&report);
    println!();
    println!("  your notes live in {}", jotbay.data_dir().display());
    println!();
    Ok(())
}

/// Desktop shortcuts, for people who reach their notes through a file manager.
///
/// Deliberately not part of setup: making one is a preference, and doing it
/// unasked puts an icon on someone's desktop they did not want.
fn cmd_shortcut(
    jotbay: &Jotbay,
    what: Option<String>,
    at: Option<PathBuf>,
) -> jotbay_core::Result<()> {
    use jotbay_core::shortcut::{self, Target};

    let targets = match what.as_deref() {
        None => vec![Target::App, Target::Notes],
        Some(text) => vec![Target::parse(text).ok_or_else(|| {
            jotbay_core::Error::Other(format!("don't know how to make a shortcut to '{text}'"))
        })?],
    };
    let location = at.unwrap_or_else(shortcut::default_location);

    println!();
    let mut made = 0;
    for target in targets {
        let source = match target {
            Target::Notes => Some(jotbay.data_dir()),
            Target::App => shortcut::locate_app(),
        };
        let Some(source) = source else {
            // Only worth a warning when the user asked for it specifically;
            // with no argument this is just "the GUI isn't installed here".
            if what.is_some() {
                render::error("could not find the Jotbay application on this machine");
            }
            continue;
        };
        match shortcut::create(target, &source, &location) {
            Ok(path) => {
                println!("  {}", path.display());
                made += 1;
            }
            Err(e) => render::error(&e.to_string()),
        }
    }

    if made == 0 {
        println!("  nothing to link");
    }
    println!();
    Ok(())
}

/// Report the background sync, and set it up if it is missing.
fn cmd_schedule() -> jotbay_core::Result<()> {
    use jotbay_core::schedule;
    println!();
    if schedule::ensure()? {
        println!("  background sync is now set up on this machine");
    } else {
        println!("  background sync is already set up");
    }
    println!("  logs: {}", schedule::log_hint());
    println!();
    Ok(())
}

/// Run the watcher in the foreground, narrating what it does.
///
/// The supervisor is the operating system's: launchd, systemd and Task
/// Scheduler all restart a process that dies and capture what it printed, so
/// there is nothing here worth reimplementing.
fn cmd_watch(jotbay: &Jotbay) -> jotbay_core::Result<()> {
    use jotbay_core::watch::{self, Event};

    println!();
    // The vault root, matching what the watcher actually walks. This printed
    // `data/` while core watched the whole repository, so the log said the
    // watcher was ignoring exactly the files it had just been fixed to see.
    println!("  watching {}", jotbay.git().root().display());
    println!("  edits sync automatically; press ctrl-c to stop");
    println!();

    watch::run(jotbay, |event, detail| {
        let text = detail.unwrap_or_default();
        match event {
            Event::Local | Event::Remote => render::watch_event(&text, false),
            Event::Failed => render::watch_event(&text, true),
        }
    })
}

fn cmd_upgrade(jotbay: &Jotbay, json: bool) -> jotbay_core::Result<()> {
    // Asking to upgrade is exactly the moment to find out what "latest" is,
    // rather than trusting a cache that may be six hours old.
    jotbay_core::update::refresh_remote_now();

    let status = jotbay.update_status();
    match (&status.latest, status.available) {
        (None, _) => {
            if !json {
                println!("  No release found. Check your connection, or sync first.");
            }
            return Err(jotbay_core::Error::Other("no release found".into()));
        }
        (Some(latest), false) => {
            // Nothing to do is a success. Reporting it as an error made the
            // window show a red failure for a machine that was up to date.
            if json {
                let outcome = jotbay_core::update::Outcome {
                    route: jotbay_core::update::route(),
                    version: status.current.clone(),
                    replaced: Vec::new(),
                    sync_restarted: false,
                    restart_app: false,
                    already_current: true,
                };
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else {
                println!("  already on {} (latest is {latest})", status.current);
            }
            return Ok(());
        }
        (Some(latest), true) => {
            if !json {
                println!();
                println!("  upgrading {} → {latest}", status.current);
            }
        }
    }

    let outcome = jotbay.upgrade()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        render::upgrade(&outcome);
    }
    Ok(())
}

fn on(value: &str) -> bool {
    matches!(value.trim().to_lowercase().as_str(), "on" | "true" | "yes" | "1")
}

fn cmd_settings(json: bool, assignment: Option<String>) -> jotbay_core::Result<()> {
    use jotbay_core::settings::{Settings, Theme};
    let mut settings = Settings::load();

    if let Some(a) = assignment {
        let (key, value) = a
            .split_once('=')
            .ok_or_else(|| jotbay_core::Error::Other("expected KEY=VALUE".into()))?;
        match key.trim() {
            "theme" => {
                settings.theme = Theme::parse(value).ok_or_else(|| {
                    jotbay_core::Error::Other("theme must be system, light or dark".into())
                })?
            }
            "verbose" => {
                settings.verbose = on(value);
            }
            "raw_activity" | "raw-activity" => {
                settings.raw_activity = on(value);
            }
            other => {
                return Err(jotbay_core::Error::Other(format!(
                    "Unknown setting '{other}'. Try theme, verbose or raw_activity."
                )))
            }
        }
        settings.save()?;
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&settings)?);
    } else {
        render::settings(&settings);
    }
    Ok(())
}

fn cmd_activity(
    jotbay: &Jotbay,
    json: bool,
    limit: usize,
    refresh: bool,
    raw: bool,
) -> jotbay_core::Result<()> {
    let settings = jotbay_core::settings::Settings::load();
    // The flag is a one-off override of the preference, not a second setting.
    let raw = raw || settings.raw_activity;

    if raw {
        let events = jotbay.activity(refresh, limit)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&events)?);
        } else {
            render::activity(&events, settings.verbose);
        }
    } else {
        let changes = jotbay.changes(refresh, limit)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&changes)?);
        } else {
            render::changes(&changes, settings.verbose);
        }
    }
    Ok(())
}

fn cmd_log(jotbay: &Jotbay, json: bool, limit: u32) -> jotbay_core::Result<()> {
    let commits = jotbay.log(limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&commits)?);
    } else {
        render::log(&commits);
    }
    Ok(())
}

fn cmd_resolve(jotbay: &Jotbay, abort: bool) -> jotbay_core::Result<()> {
    if abort {
        jotbay.abort_rebase()?;
        println!("  rebase aborted; the working tree is clean again");
        return Ok(());
    }

    let status = jotbay.status(false)?;
    if !status.rebase_in_progress {
        println!("  nothing to resolve");
        return Ok(());
    }

    println!("  {} conflicted file(s):", status.conflicts.len());
    for f in &status.conflicts {
        println!("    {f}");
    }
    println!();
    println!("  `jotbay sync` applies the keep-both-sides policy automatically.");
    println!("  `jotbay resolve --abort` discards the rebase instead.");
    Ok(())
}
