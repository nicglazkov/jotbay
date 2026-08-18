//! Terminal presentation.
//!
//! Tables are laid out by hand rather than with a table crate: the columns are
//! few and fixed, and controlling padding directly keeps the output aligned
//! with the GUI's information design.

use owo_colors::OwoColorize;
use jotbay_core::limits::{FileWarning, Severity};
use jotbay_core::{
    ActivityEvent, CommitInfo, EventKind, NodeHealth, NodeStatus, SyncReport, JotbayStatus,
    SYNC_INTERVAL_SECS,
};

pub fn human_age(secs: i64) -> String {
    match secs {
        s if s < 0 => "just now".into(),
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    }
}

fn colored_health(h: NodeHealth) -> String {
    match h {
        NodeHealth::Healthy => h.glyph().green().to_string(),
        // Cyan, not yellow: "behind" self-heals on the node's next pull, and
        // painting it as a warning made every push look like a problem.
        NodeHealth::Behind => h.glyph().cyan().to_string(),
        NodeHealth::Diverged => h.glyph().yellow().to_string(),
        NodeHealth::Stale => h.glyph().bright_black().to_string(),
        // Dimmed rather than red. There is nothing to fix, and colouring an
        // ordinary commute as a failure is how red stops meaning anything.
        NodeHealth::Offline => h.glyph().bright_black().to_string(),
        // Amber, not grey. A day of silence is a problem worth looking at.
        NodeHealth::Missing => h.glyph().yellow().to_string(),
        NodeHealth::Error => h.glyph().red().to_string(),
    }
}

pub fn status(s: &JotbayStatus) {
    println!();
    println!(
        "  {}  {}",
        "jotbay".bold(),
        s.root.bright_black()
    );

    // Headline state.
    let headline = if s.rebase_in_progress {
        format!("{} {} conflicted file(s) awaiting resolution", "✖".red(), s.conflicts.len())
    } else if s.is_clean() {
        format!("{} this machine is in sync", "●".green())
    } else {
        let mut bits = Vec::new();
        if !s.dirty_files.is_empty() {
            bits.push(format!("{} uncommitted", s.dirty_files.len()));
        }
        if s.ahead > 0 {
            bits.push(format!("{} to push", s.ahead));
        }
        if s.behind > 0 {
            bits.push(format!("{} to pull", s.behind));
        }
        format!("{} {}", "◐".yellow(), bits.join(", "))
    };

    println!("  {headline}");
    println!(
        "  {}",
        format!(
            "{} · {} · {} file(s) in data/",
            s.branch, s.head_short, s.data_files
        )
        .bright_black()
    );

    if !s.dirty_files.is_empty() {
        println!();
        for f in s.dirty_files.iter().take(10) {
            println!("    {} {}", "~".yellow(), f);
        }
        if s.dirty_files.len() > 10 {
            println!("    {}", format!("and {} more", s.dirty_files.len() - 10).bright_black());
        }
    }

    if !s.conflicts.is_empty() {
        println!();
        println!("  {}", "conflicted:".red().bold());
        for f in &s.conflicts {
            println!("    {} {}", "✖".red(), f);
        }
    }

    file_warnings(&s.warnings);
    if let Some(latest) = &s.update_available {
        println!();
        println!(
            "  {} {}",
            "↑".cyan().bold(),
            format!("Update available: {latest}. Run `jotbay upgrade`.").cyan()
        );
    }
    nodes(&s.nodes, &s.head);
    println!();
}

/// Files that will not sync, or that will cost more than the user expects.
pub fn file_warnings(warnings: &[FileWarning]) {
    if warnings.is_empty() {
        return;
    }

    let blocked: Vec<_> = warnings.iter().filter(|w| w.severity == Severity::Blocked).collect();
    let other: Vec<_> = warnings.iter().filter(|w| w.severity != Severity::Blocked).collect();

    if !blocked.is_empty() {
        println!();
        println!(
            "  {} {}",
            "✖".red().bold(),
            format!(
                "{} file{} cannot be synced and {} not committed:",
                blocked.len(),
                if blocked.len() == 1 { "" } else { "s" },
                if blocked.len() == 1 { "was" } else { "were" }
            )
            .red()
            .bold()
        );
        for w in &blocked {
            println!("    {} {}  {}", "•".red(), w.path.bold(), w.human_size().red());
        }
        advice(Severity::Blocked);
    }

    if !other.is_empty() {
        println!();
        println!("  {} large file(s):", "⚠".yellow());
        for w in &other {
            let size = match w.severity {
                Severity::Warning => w.human_size().yellow().to_string(),
                _ => w.human_size().bright_black().to_string(),
            };
            println!("    {} {}  {}", "•".yellow(), w.path, size);
        }
        advice(Severity::Advisory);
    }
}

/// Print the one canonical advice string for a severity, wrapped to the block
/// the rest of this module indents to. The words live in `jotbay_core::limits`
/// so the CLI and both GUIs cannot say different things about the same file.
fn advice(severity: Severity) {
    const WIDTH: usize = 74;

    let mut line = String::new();
    for word in severity.advice().split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > WIDTH {
            println!("    {}", line.bright_black());
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        println!("    {}", line.bright_black());
    }
}

pub fn nodes(nodes: &[NodeStatus], local_head: &str) {
    if nodes.is_empty() {
        println!();
        println!("  {}", "no nodes have reported in yet".bright_black());
        return;
    }

    let w_host = nodes.iter().map(|n| n.hostname.len()).max().unwrap_or(8).max(8);
    let w_os = nodes.iter().map(|n| n.os.len()).max().unwrap_or(5).max(5);

    println!();
    println!(
        "  {}",
        format!(
            "  {:<w_host$}  {:<w_os$}  {:<9}  {:<10}  {}",
            "NODE", "OS", "COMMIT", "LAST SYNC", "STATE",
            w_host = w_host,
            w_os = w_os
        )
        .bright_black()
    );

    for n in nodes {
        let health = n.health(SYNC_INTERVAL_SECS, local_head);
        let head = if n.head.len() >= 7 { &n.head[..7] } else { &n.head };

        let mut state = health.label().to_string();
        if n.behind > 0 {
            state = format!("{state} ({} behind)", n.behind);
        } else if n.ahead > 0 {
            state = format!("{state} ({} ahead)", n.ahead);
        }
        if n.dirty > 0 {
            state = format!("{state} · {} dirty", n.dirty);
        }

        let state = match health {
            NodeHealth::Healthy => state.green().to_string(),
            NodeHealth::Behind => state.cyan().to_string(),
            NodeHealth::Diverged => state.yellow().to_string(),
            NodeHealth::Stale => state.bright_black().to_string(),
            NodeHealth::Offline => state.bright_black().to_string(),
            NodeHealth::Missing => state.yellow().to_string(),
            NodeHealth::Error => state.red().to_string(),
        };

        println!(
            "  {} {:<w_host$}  {:<w_os$}  {:<9}  {:<10}  {}",
            colored_health(health),
            n.hostname,
            n.os,
            head,
            human_age(n.age_secs()),
            state,
            w_host = w_host,
            w_os = w_os
        );

        if let Some(err) = &n.last_error {
            println!("    {} {}", "└".bright_black(), err.red());
        }
    }
}

pub fn sync_report(r: &SyncReport) {
    if r.skipped_locked {
        println!("  {} another sync is already running", "·".bright_black());
        return;
    }

    if r.did_nothing() {
        println!("  {} already in sync", "●".green());
        // Still report blocked files. Dropping one oversized video and nothing
        // else IS a no-op sync, and staying silent there would hide the single
        // case this check exists for.
        file_warnings(&r.warnings);
        return;
    }

    if r.committed {
        println!("  {} committed local changes", "✓".green());
    }
    if r.pulled > 0 {
        println!("  {} pulled {} commit(s)", "↓".cyan(), r.pulled);
    }

    for c in &r.conflicts {
        match &c.kept_copy {
            Some(copy) => {
                println!("  {} conflict in {}", "⚠".yellow(), c.path.bold());
                println!("    {} Both versions kept. Yours is at {}", "└".bright_black(), copy.cyan());
            }
            None => {
                println!("  {} {} kept the surviving version", "⚠".yellow(), c.path.bold());
            }
        }
    }

    if r.pushed {
        println!("  {} pushed", "↑".cyan());
    }
    if !r.head_short.is_empty() {
        println!("  {} now at {}", "●".green(), r.head_short.bold());
    }

    file_warnings(&r.warnings);
}

pub fn activity(events: &[ActivityEvent], verbose: bool) {
    println!();
    if events.is_empty() {
        println!("  {}", "nothing has happened yet".bright_black());
        println!("  {}", "syncs that change nothing are not recorded".bright_black());
        println!();
        return;
    }

    for group in collapse(events) {
        let e = group.first;
        let glyph = match e.kind {
            EventKind::Changed => e.kind.glyph().cyan().to_string(),
            EventKind::Conflict => e.kind.glyph().yellow().to_string(),
            EventKind::Error => e.kind.glyph().red().to_string(),
            EventKind::Offline => e.kind.glyph().bright_black().to_string(),
        };
        let summary = match e.kind {
            EventKind::Error => e.summary.red().to_string(),
            EventKind::Offline => e.summary.bright_black().to_string(),
            _ => e.summary.clone(),
        };
        // Repeats are collapsed rather than printed N times: the same blocked
        // file reported on every ten-minute sync filled the pane with identical
        // rows and buried everything else.
        let repeat = if group.count > 1 {
            format!(" {}", format!("(x{})", group.count).bright_black())
        } else {
            String::new()
        };

        println!(
            "  {} {}{}",
            glyph,
            summary,
            repeat
        );
        println!(
            "      {}  {}",
            e.hostname.bright_black(),
            human_age(e.age_secs()).bright_black()
        );

        for f in e.files.iter().take(if verbose { usize::MAX } else { 5 }) {
            println!("      {} {}", "·".bright_black(), f.bright_black());
        }
        if !verbose && e.files.len() > 5 {
            println!(
                "      {}",
                format!("· and {} more", e.files.len() - 5).bright_black()
            );
        }

        if verbose {
            if let Some(d) = &e.detail {
                for line in d.lines() {
                    println!("      {} {}", "│".bright_black(), line.bright_black());
                }
            }
        }
    }

    if !verbose && events.iter().any(|e| e.detail.is_some()) {
        println!();
        println!(
            "  {}",
            "run with `jotbay settings verbose=on` to see the raw detail".bright_black()
        );
    }
    println!();
}

/// The feed as changes rather than as machine chatter.
pub fn changes(items: &[jotbay_core::changes::Change], verbose: bool) {
    use jotbay_core::changes::ChangeKind;

    println!();
    if items.is_empty() {
        println!("  {}", "nothing has happened yet".bright_black());
        println!();
        return;
    }

    for c in items {
        let glyph = match c.kind {
            ChangeKind::Updated => c.kind.glyph().cyan().to_string(),
            ChangeKind::Conflict => c.kind.glyph().yellow().to_string(),
            ChangeKind::Offline => c.kind.glyph().bright_black().to_string(),
            ChangeKind::Problem => c.kind.glyph().red().to_string(),
        };
        let summary = match c.kind {
            ChangeKind::Problem => c.summary.red().to_string(),
            ChangeKind::Offline => c.summary.bright_black().to_string(),
            _ => c.summary.clone(),
        };
        // "still happening" rather than "happened 34 times": the count is
        // there to show persistence, not to be counted.
        let repeat = if c.repeats > 1 {
            format!(" {}", format!("×{}", c.repeats).bright_black())
        } else {
            String::new()
        };
        println!("  {glyph} {summary}{repeat}");

        let mut meta: Vec<String> = Vec::new();
        match (&c.origin, c.machines.len()) {
            // The machine that made it, when the commit event is still in the
            // buffer to say so.
            (Some(origin), _) => meta.push(origin.clone()),
            // Otherwise name who has it rather than implying authorship: this
            // machine only received the change and cannot say who made it.
            (None, 1) => meta.push(c.machines[0].clone()),
            (None, _) => {}
        }
        if c.machines.len() > 1 {
            meta.push(format!("on {} machines", c.machines.len()));
        }
        meta.push(human_age((time::OffsetDateTime::now_utc() - c.at).whole_seconds()));
        println!("      {}", meta.join("  ·  ").bright_black());

        if verbose {
            for f in &c.files {
                println!("      {} {}", "·".bright_black(), f.bright_black());
            }
            if let Some(d) = &c.detail {
                println!("      {}", d.bright_black());
            }
        }
    }

    if !verbose {
        println!();
        println!(
            "  {}",
            "run `jotbay activity --raw` to see what each machine did".bright_black()
        );
    }
    println!();
}

struct Group<'a> {
    first: &'a ActivityEvent,
    count: usize,
}

/// Fold runs of the same machine reporting the same thing into one row.
fn collapse(events: &[ActivityEvent]) -> Vec<Group<'_>> {
    let mut out: Vec<Group> = Vec::new();
    for e in events {
        match out.last_mut() {
            Some(g) if g.first.hostname == e.hostname && g.first.summary == e.summary => {
                g.count += 1;
            }
            _ => out.push(Group { first: e, count: 1 }),
        }
    }
    out
}

/// The same facts the settings panels show.
///
/// The background sync block earns its place: a machine can be fully upgraded
/// and still sync with the previous version, because replacing the binaries
/// does not restart the watcher. That state is invisible everywhere else, and
/// the version it publishes is the one other machines believe.
pub fn about(a: &jotbay_core::about::About) {
    println!();
    println!("  {} {}", "jotbay".bold(), a.version.bold());
    println!("    {} on {} {}", a.hostname, a.os, a.arch);
    println!();

    println!("  {}", "notes".bold());
    println!("    folder    {}", a.notes.display());
    if a.root != a.notes {
        println!("    vault     {}", a.root.display());
    }
    println!("    files     {}", a.files);
    println!("    branch    {}", a.branch);
    match &a.remote {
        Some(r) => println!("    remote    {r}"),
        None => println!("    remote    {}", "none".bright_black()),
    }
    println!();

    println!("  {}", "background sync".bold());
    if a.sync.scheduled {
        match a.sync.last_report_secs {
            Some(secs) => println!("    schedule  installed, last reported {}", human_age(secs)),
            None => println!("    schedule  installed, has never reported"),
        }
    } else {
        println!("    schedule  {}", "not installed, run: jotbay schedule".yellow());
    }
    if let Some(running) = &a.sync.running_version {
        if a.sync.restart_needed {
            println!("    running   {}", format!("{running}, older than the installed {}", a.version).yellow());
            println!("    {}", "restart the background sync to pick up the new version".yellow());
        } else {
            println!("    running   {running}");
        }
    }
    println!();

    println!("  {}", "updates".bold());
    match (&a.update_available, a.upgrade_in_place) {
        (Some(v), true) => {
            println!("    {}", format!("version {v} is available, run: jotbay upgrade").green())
        }
        // Naming the command that cannot work here would send someone to an
        // error. The engine already knows which one applies.
        (Some(v), false) => {
            println!("    {}", format!("version {v} is available").green());
            if let Some(how) = &a.upgrade_instructions {
                println!("    {}", how.bright_black());
            }
        }
        (None, _) => println!("    up to date"),
    }
    println!("    source    {}", a.tool_repo.bright_black());
    println!();

    println!("  {}", a.config_path.display().to_string().bright_black());
    println!();
}

pub fn settings(s: &jotbay_core::settings::Settings) {
    println!();
    println!("  {}", "settings".bold());
    println!("    theme     {}", s.theme.as_str());
    println!("    verbose   {}", if s.verbose { "on" } else { "off" });
    println!();
    println!("  {}", jotbay_core::settings::settings_path().display().to_string().bright_black());
    println!("  {}", "these are per-machine and never sync".bright_black());
    println!();
}

pub fn log(commits: &[CommitInfo]) {
    println!();
    for c in commits {
        let who = c.node.clone().unwrap_or_else(|| c.author.clone());
        println!(
            "  {}  {}  {}",
            c.short.yellow(),
            c.timestamp.bright_black(),
            c.subject
        );
        println!("        {}", who.bright_black());
    }
    println!();
}

/// A failure that is only the absence of a network.
///
/// Dimmed and without a "✖", because there is nothing wrong and nothing for
/// the reader to do. Local work is already committed by the time any network
/// call runs, so the sentence can promise that safely.
pub fn offline_notice() {
    println!();
    println!(
        "  {} {}",
        "◌".bright_black(),
        "Offline. Your work is saved here and will sync when the network is back."
            .bright_black()
    );
    println!();
}

/// What the upgrade did, in the terms a person cares about.
pub fn upgrade(o: &jotbay_core::update::Outcome) {
    use jotbay_core::update::Route;

    let how = match o.route {
        Route::Binaries => "replaced the binaries",
        Route::HomebrewCask => "upgraded the Homebrew cask",
        Route::AptPackage => "installed the new package",
        Route::WindowsInstaller => "replaced the program files",
        Route::MacAppBundle => "replaced Jotbay.app",
    };
    println!("  {} {}", "✓".green(), format!("now on {} · {how}", o.version));

    // Said either way. Silence about the background sync is what let three
    // machines keep running the old version after a successful upgrade.
    if o.sync_restarted {
        println!("  {} background sync restarted", "✓".green());
    } else {
        println!(
            "  {} {}",
            "·".bright_black(),
            "no background sync is set up here, run `jotbay schedule`".bright_black()
        );
    }
    if o.restart_app {
        println!(
            "  {} {}",
            "·".bright_black(),
            "quit and reopen the Jotbay window to use the new version".bright_black()
        );
    }
    println!();
}

/// Search results. Name matches first, because that is usually the answer.
pub fn hits(hits: &[jotbay_core::notes::Hit], query: &str) {
    println!();
    if hits.is_empty() {
        println!("  {}", format!("nothing matches {query}").bright_black());
        println!();
        return;
    }
    for h in hits {
        let mark = if h.name_match { "◆".cyan().to_string() } else { "·".bright_black().to_string() };
        println!("  {mark} {}", h.path);
        if let (Some(line), Some(excerpt)) = (h.line, &h.excerpt) {
            println!("      {}", format!("{line}: {excerpt}").bright_black());
        }
    }
    println!();
}

/// One note over time.
pub fn history(versions: &[jotbay_core::notes::Version], file: &str) {
    println!();
    println!("  {}", file.bold());
    if versions.is_empty() {
        println!("  {}", "no history yet, it has never been committed".bright_black());
        println!();
        return;
    }
    for v in versions {
        let when = v.at.get(..16).unwrap_or(&v.at).replace('T', " ");
        let who = v.machine.clone().unwrap_or_else(|| "-".into());
        let what = if v.deleted { " deleted".red().to_string() } else { String::new() };
        println!("  {}  {}  {}{}", v.short.cyan(), when.bright_black(), who, what);
    }
    println!();
    println!(
        "  {}",
        format!("restore one with: jotbay restore {file} <version>").bright_black()
    );
    println!();
}

pub fn deleted(gone: &[jotbay_core::notes::Deleted]) {
    println!();
    if gone.is_empty() {
        println!("  {}", "nothing has been deleted".bright_black());
        println!();
        return;
    }
    for d in gone {
        let when = d.at.get(..16).unwrap_or(&d.at).replace('T', " ");
        let who = d.machine.clone().unwrap_or_else(|| "-".into());
        println!("  {}  {}  {}", d.path, when.bright_black(), who.bright_black());
    }
    println!();
    println!(
        "  {}",
        "bring one back with: jotbay restore <file>".bright_black()
    );
    println!();
}

/// Says where the file went, and that nothing has been sent yet.
///
/// Restoring and creating both leave an ordinary uncommitted change, which the
/// watcher picks up like any other edit. Saying so avoids the question of
/// whether something still needs pushing.
pub fn wrote(path: &std::path::Path, verb: &str) {
    println!();
    println!("  {} {verb} {}", "✓".green(), path.display());
    println!("  {}", "it will sync with the next change".bright_black());
    println!();
}

pub fn settled() {
    println!();
    println!("  {} settled. It will sync with the next change.", "✓".green());
    println!();
}

/// Conflict copies waiting for a decision.
pub fn conflict_pairs(pairs: &[jotbay_core::pairs::ConflictPair]) {
    println!();
    if pairs.is_empty() {
        println!("  {}", "no conflicts waiting".bright_black());
        println!();
        return;
    }
    for p in pairs {
        let when = p.at.as_deref().map(|a| a.get(..16).unwrap_or(a).replace('T', " ")).unwrap_or_default();
        let who = p.machine.clone().unwrap_or_else(|| "another machine".into());
        println!("  {} {}", "⚠".yellow(), p.original.bold());
        if p.identical {
            println!("      {}", format!("both versions are identical · {who} · {when}").bright_black());
        } else {
            println!("      {}", format!("also edited on {who} · {when}").bright_black());
            println!("      {}", format!("their copy: {}", p.copy).bright_black());
        }
    }
    println!();
    println!(
        "  {}",
        "settle one with: jotbay conflicts <copy> --settle keep-current | keep-copy | keep-both"
            .bright_black()
    );
    println!();
}

pub fn error(msg: &str) {
    eprintln!("  {} {}", "✖".red(), msg);
}

pub fn capabilities(c: &jotbay_core::setup::SetupCapabilities) {
    println!();
    println!("  {}", "no vault on this machine yet".bold());
    println!();
    println!("  {:<10} {}", "git", if c.git { "yes".green().to_string() } else { "missing".red().to_string() });
    println!(
        "  {:<10} {}",
        "gh",
        match (c.gh, c.gh_authenticated, &c.login) {
            (true, true, Some(l)) => format!("signed in as {l}").green().to_string(),
            (true, true, None) => "signed in".green().to_string(),
            (true, false, _) => "installed, not signed in".yellow().to_string(),
            _ => "missing".yellow().to_string(),
        }
    );
    println!();

    if c.gh_authenticated {
        println!("  {}", "create a private repository and start syncing:".bright_black());
        println!("    jotbay init --create jotbay");
    } else {
        println!("  {}", "sign in first to have one made for you:".bright_black());
        println!("    gh auth login");
    }
    println!();
    println!("  {}", "or use one you already have:".bright_black());
    println!("    jotbay init --clone <url>");
    println!("    jotbay init --adopt <path>");
    println!();
    println!("  {}", format!("default location: {}", c.default_location).bright_black());
    println!();
}

/// One timestamped line from the watcher.
///
/// Timestamped because this output is read hours later in a log file, where
/// "pushed" with no time attached answers nothing.
pub fn watch_event(text: &str, failed: bool) {
    // UTC: the `local-offset` feature is not enabled, and a log that is
    // consistently UTC beats one that silently falls back to it.
    let now = time::OffsetDateTime::now_utc();
    let stamp = format!("{:02}:{:02}:{:02}Z", now.hour(), now.minute(), now.second());
    if failed {
        println!("  {} {} {}", stamp.dimmed(), "\u{2716}".red(), text);
    } else {
        println!("  {} {} {}", stamp.dimmed(), "\u{2713}".green(), text);
    }
}
