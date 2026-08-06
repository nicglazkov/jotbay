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
        };
        let summary = match e.kind {
            EventKind::Error => e.summary.red().to_string(),
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
