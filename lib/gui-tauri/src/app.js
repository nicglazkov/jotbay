// Jotbay front end.
//
// All state comes from the Rust side via `invoke`; nothing about sync is
// decided here. `withGlobalTauri` is enabled in tauri.conf.json so this runs
// as a plain script with no bundler or build step.

const invoke = window.__TAURI__.core.invoke;

const SYNC_INTERVAL = 300; // seconds; matches the scheduler
let state = { status: null, activity: [], syncing: false,
              settings: { theme: "system", verbose: false },
              about: null,
              expanded: new Set() };

/// "1 conflict" rather than "1 conflict(s)". Parenthesised plurals are the
/// tell of a string that was never read aloud.
function plural(n, word) { return n === 1 ? word : `${word}s`; }

const EVENT_GLYPH = { changed: "↕", conflict: "⚠", error: "✖" };


// WebView2 does not repaint an embedded view when Windows switches theme, so
// the CSS media query alone leaves the window in whichever palette it launched
// with. Mirroring the query onto an explicit attribute gives the stylesheet
// something that definitely changes. Harmless where the media query already
// works (WebKitGTK), and a no-op if the engine never fires the event.
const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

function applyTheme() {
  const pref = state.settings.theme || "system";
  const dark = pref === "system" ? darkQuery.matches : pref === "dark";
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  forceRepaint();
}

// Setting the attribute is not always enough on WebView2. Going light → dark
// left the summary bar, toast strip and Machines pane painted light while the
// Activity pane and pane headers went dark, and the Machines rows stayed light
// even after being replaced wholesale by the 20s poll. `.node` and `.event` are
// structurally identical (same flex, padding, `var(--border)`, no background)
// and sit in sibling panes, so a cascade explanation does not survive contact
// with that asymmetry: it is stale rasterisation of some composited layers, not
// unresolved custom properties. Detaching the root forces the whole tree to be
// laid out and rasterised again.
//
// Costs one frame on a theme change only. Verified harmless on WebKitGTK and
// macOS; NOT yet confirmed to fix WebView2, see REINSTALL-WINDOWS.md.
function forceRepaint() {
  const root = document.documentElement;
  const previous = root.style.display;
  root.style.display = "none";
  void root.offsetHeight; // flush layout while detached
  root.style.display = previous;
}

applyTheme();
darkQuery.addEventListener("change", () => applyTheme());

// --- helpers ---------------------------------------------------------------

const $ = (id) => document.getElementById(id);

function humanAge(seconds) {
  if (seconds < 0) return "just now";
  if (seconds < 60) return `${Math.floor(seconds)}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

function ageSeconds(rfc3339) {
  return (Date.now() - new Date(rfc3339).getTime()) / 1000;
}

/// Kept in step with NodeHealth in the Rust core. `behind_local` is the
/// read-time ancestry annotation computed by core's read_all: a node that is
/// strictly behind this machine is not "diverged", it just has not pulled yet.
function nodeHealth(node, localHead) {
  if (node.last_error) return "error";
  if (ageSeconds(node.last_sync) > SYNC_INTERVAL * 3) return "stale";
  if (node.head !== localHead) return node.behind_local ? "behind" : "diverged";
  return "healthy";
}

const HEALTH_LABEL = {
  healthy: "in sync",
  behind: "behind",
  diverged: "diverged",
  stale: "not answering",
  error: "error",
};

// Text from git, commit subjects, hostnames, error strings, is never
// interpolated into HTML without escaping. A commit subject is attacker-
// influenced in the sense that it arrives from another machine, so it is
// treated as untrusted regardless of the repo being private.
//
// Quotes are escaped as well as angle brackets. Every interpolation below sits
// in a text position today, where quotes are harmless, but escaping them keeps
// a future edit that moves one into an attribute from silently opening a hole.
function esc(text) {
  return String(text ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function toast(message, kind = "info") {
  const el = $("toast");
  if (!message) {
    el.hidden = true;
    return;
  }
  el.textContent = message;
  el.dataset.kind = kind;
  el.hidden = false;
}

// --- rendering -------------------------------------------------------------

function render() {
  const s = state.status;
  if (!s) return;

  $("repo-path").textContent = s.root;
  $("m-files").textContent = s.data_files;
  $("m-nodes").textContent = s.nodes.length;
  $("m-commit").textContent = s.head_short || "-";
  $("spinner").hidden = !state.syncing;
  $("btn-sync").disabled = state.syncing;

  const isClean =
    s.dirty_files.length === 0 && s.ahead === 0 && s.behind === 0 && !s.rebase_in_progress;

  let health = "healthy";
  let headline = "This machine is in sync";

  if (s.rebase_in_progress) {
    health = "error";
    headline = `${s.conflicts.length} file(s) need resolving`;
  } else if (s.nodes.some((n) => n.last_error)) {
    health = "error";
    headline = "A machine reported an error";
  } else if (!isClean) {
    health = "diverged";
    const parts = [];
    if (s.dirty_files.length) parts.push(`${s.dirty_files.length} uncommitted`);
    if (s.ahead) parts.push(`${s.ahead} to push`);
    if (s.behind) parts.push(`${s.behind} to pull`);
    headline = parts.join(", ");
  }

  $("headline").textContent = headline;
  $("overall-dot").dataset.health = health;

  const banner = $("conflict-banner");
  banner.hidden = !s.rebase_in_progress;
  if (s.rebase_in_progress) {
    $("conflict-count").textContent = `${s.conflicts.length} file(s) awaiting resolution.`;
  }

  const upd = $("update-banner");
  upd.hidden = !s.update_available;
  if (s.update_available) {
    $("update-text").textContent = `Version ${s.update_available} is available.`;
  }

  renderLimits(s.warnings || []);
  renderNodes(s);
  renderActivity();
}

function humanSize(bytes) {
  const mb = bytes / (1024 * 1024);
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  if (mb >= 1) return `${Math.round(mb)} MB`;
  return `${Math.round(bytes / 1024)} KB`;
}

// Files that will not sync, or that cost more than the user expects. Blocked
// files matter most: they sit in Jotbay looking synced, and without this the
// only symptom is their absence on the other machines.
function renderLimits(warnings) {
  const host = $("limit-banner");
  if (!warnings.length) {
    host.hidden = true;
    return;
  }

  const blocked = warnings.filter((w) => w.severity === "blocked");
  const large = warnings.filter((w) => w.severity !== "blocked");
  const rows = (list) =>
    list
      .map(
        (w) =>
          `<div class="limit-row"><span class="limit-name">${esc(
            w.path.split("/").pop()
          )}</span><span class="limit-size">${humanSize(w.bytes)}</span></div>`
      )
      .join("");

  // The advice text comes down on each warning from jotbay_core::limits rather
  // than living here. Keeping a copy in the UI is how this pane went on
  // recommending Git LFS long after core stopped, and LFS would corrupt files
  // during conflict resolution.
  const adviceFor = (list) => esc(list.find((w) => w.advice)?.advice ?? "");

  let html = "";
  if (blocked.length) {
    html += `<div class="limit-group" data-kind="blocked">
      <strong>✖ ${blocked.length} file${blocked.length === 1 ? "" : "s"} can't be synced</strong>
      ${rows(blocked)}
      <p>${adviceFor(blocked)}</p>
    </div>`;
  }
  if (large.length) {
    html += `<div class="limit-group" data-kind="large">
      <strong>⚠ ${large.length} large file${large.length === 1 ? "" : "s"}</strong>
      ${rows(large)}
      <p>${adviceFor(large)}</p>
    </div>`;
  }

  host.innerHTML = html;
  host.dataset.kind = blocked.length ? "blocked" : "large";
  host.hidden = false;
}

function renderNodes(s) {
  $("nodes-count").textContent = s.nodes.length;
  const host = $("nodes");

  if (!s.nodes.length) {
    host.innerHTML = `<div class="empty">
      <div class="empty-title">No machines yet</div>
      <div class="empty-detail">Each machine reports in the first time it syncs.</div>
    </div>`;
    return;
  }

  host.innerHTML = s.nodes
    .map((n) => {
      const health = nodeHealth(n, s.head);
      // The health label used to be prepended unconditionally, which read
      // "behind - 1 behind": the count already says it, and says it better.
      // Keep the label only when nothing more precise is available.
      const detail = [];
      if (n.behind) detail.push(`${n.behind} behind`);
      if (n.ahead) detail.push(`${n.ahead} ahead`);
      if (n.dirty) detail.push(`${n.dirty} uncommitted`);
      if (n.conflicts_resolved) {
        detail.push(`${n.conflicts_resolved} ${plural(n.conflicts_resolved, "conflict")} kept`);
      }
      if (!detail.length) detail.push(HEALTH_LABEL[health]);

      return `<div class="node">
        <span class="dot node-dot" data-health="${health}"></span>
        <div class="node-main">
          <div class="node-name">
            <strong>${esc(n.hostname)}</strong>
            <span class="tag">${esc(n.os)}</span>
          </div>
          <div class="node-detail">${esc(detail.join(" · "))}</div>
          ${n.last_error ? `<div class="node-error">${esc(n.last_error)}</div>` : ""}
        </div>
        <div class="node-right">
          <div class="node-age">${humanAge(ageSeconds(n.last_sync))}</div>
          <div class="node-head">${esc(n.head.slice(0, 7))}</div>
        </div>
      </div>`;
    })
    .join("");
}

function renderActivity() {
  const host = $("log");
  const groups = collapseEvents(state.activity);
  $("log-count").textContent = state.activity.length;

  if (!groups.length) {
    host.innerHTML = `<div class="empty">
      <div class="empty-title">Nothing has happened yet</div>
      <div class="empty-detail">Syncs that change nothing aren't recorded,
        so this stays quiet until something moves.</div>
    </div>`;
    return;
  }

  host.innerHTML = groups
    .map((g, i) => {
      const e = g.first;
      // `files` is Option<Vec<String>> on the Rust side, so it arrives as null
      // whenever an event touched no files, which is exactly what an *error*
      // event is. Reading .length off it threw, and one throw here empties the
      // whole activity pane, so a machine reporting a failure erased the list
      // that was supposed to show the failure.
      const files = e.files || [];
      const expandable = files.length > 0 || (state.settings.verbose && e.detail);
      const open = state.expanded.has(i);
      const repeat = g.count > 1 ? `<span class="event-repeat">x${g.count}</span>` : "";

      let body = "";
      if (open) {
        if (files.length) {
          body += `<div class="event-files">${files
            .map((f) => `<div>${esc(f)}</div>`)
            .join("")}</div>`;
        }
        // Raw git output is hidden unless asked for: the GH007 failure is five
        // lines of remote: error:  that means nothing to most people.
        if (state.settings.verbose && e.detail) {
          body += `<div class="event-detail">${esc(e.detail)}</div>`;
        }
      }

      return `<div class="event">
        <span class="event-glyph" data-kind="${e.kind}">${EVENT_GLYPH[e.kind] || "·"}</span>
        <div class="event-main">
          <div class="event-summary" data-kind="${e.kind}">${esc(e.summary)} ${repeat}</div>
          <div class="event-meta">
            <span class="who">${esc(e.hostname)}</span>
            <span class="when">${humanAge(ageSeconds(e.at))}</span>
            ${expandable ? `<button class="event-toggle" data-i="${i}">${open ? "▾ hide" : "▸ details"}</button>` : ""}
          </div>
          ${body}
        </div>
      </div>`;
    })
    .join("");

  host.querySelectorAll(".event-toggle").forEach((b) =>
    b.addEventListener("click", () => {
      const i = Number(b.dataset.i);
      state.expanded.has(i) ? state.expanded.delete(i) : state.expanded.add(i);
      renderActivity();
    })
  );
}

// Fold runs of the same machine reporting the same thing. The same blocked file
// reported on every ten-minute sync produced a column of identical rows.
function collapseEvents(events) {
  const out = [];
  for (const e of events) {
    const last = out[out.length - 1];
    if (last && last.first.hostname === e.hostname && last.first.summary === e.summary) {
      last.count += 1;
    } else {
      out.push({ first: e, count: 1 });
    }
  }
  return out;
}


// --- first run -------------------------------------------------------------
//
// An installed app opens here, because a .msi or .dmg cannot clone a private
// repository. Three routes, and the one that needs no git knowledge is offered
// first, but only when gh can actually deliver it, so nobody picks an option
// that then fails.

let frMode = null;

async function showFirstRunIfNeeded() {
  let state;
  try {
    state = await invoke("get_setup_state");
  } catch {
    return false; // fall through to the main UI, which will show its own error
  }
  if (state.has_vault) return false;

  const caps = state.capabilities;
  $("fr-location").value = caps.default_location;

  const createBtn = $("fr-create");
  if (!caps.gh_authenticated) {
    // Say why rather than just greying out: the remedy is one command, and
    // nothing else on this screen can hint at it.
    createBtn.disabled = true;
    $("fr-create-note").textContent = caps.gh
      ? "Sign in first: run gh auth login in a terminal, then reopen Jotbay."
      : "Needs the GitHub CLI (gh) installed and signed in.";
  } else if (caps.login) {
    $("fr-create-note").textContent =
      `Makes a new private repository under ${caps.login} and starts syncing.`;
  }
  if (!caps.git) {
    for (const id of ["fr-create", "fr-clone", "fr-adopt"]) $(id).disabled = true;
    frStatus("git is not installed on this machine. Install it, then reopen Jotbay.", "error");
  }

  $("firstrun").hidden = false;
  $("app").hidden = true;
  return true;
}

function frStatus(message, kind = "info") {
  const el = $("fr-status");
  if (!message) { el.hidden = true; return; }
  el.textContent = message;
  el.dataset.kind = kind;
  el.hidden = false;
}

function frSelect(mode) {
  frMode = mode;
  for (const id of ["fr-create", "fr-clone", "fr-adopt"]) {
    $(id).setAttribute("aria-selected", String($(id).dataset.mode === mode));
  }
  const label = $("fr-label");
  const value = $("fr-value");
  value.hidden = mode === "adopt";
  label.hidden = mode === "adopt";
  // Adopting picks the clone itself, not a place to put one.
  $("fr-location-label").textContent = mode === "adopt" ? "Existing folder" : "Location";

  if (mode === "create") {
    label.textContent = "Repository name";
    value.value = "jotbay-notes";
    value.placeholder = "jotbay-notes";
  } else if (mode === "clone") {
    label.textContent = "Repository URL";
    value.value = "";
    value.placeholder = "https://github.com/you/your-notes.git";
  }
  $("fr-detail").hidden = false;
  frStatus("");
  if (mode !== "adopt") value.focus();
}

$("fr-create").addEventListener("click", () => frSelect("create"));
$("fr-clone").addEventListener("click", () => frSelect("clone"));
$("fr-adopt").addEventListener("click", () => frSelect("adopt"));
$("fr-back").addEventListener("click", () => { $("fr-detail").hidden = true; frStatus(""); });

$("fr-browse").addEventListener("click", async () => {
  // A text field for a filesystem path is not something to put in front of
  // someone who chose the GUI precisely to avoid typing paths.
  try {
    const chosen = await window.__TAURI__.dialog.open({
      directory: true,
      defaultPath: $("fr-location").value || undefined,
    });
    if (chosen) $("fr-location").value = chosen;
  } catch (e) {
    frStatus(String(e), "error");
  }
});

$("fr-go").addEventListener("click", async () => {
  const location = $("fr-location").value.trim();
  const value = $("fr-value").value.trim();
  if (!location) return frStatus("Choose where your notes should live.", "error");
  if (frMode === "create" && !value) return frStatus("Give the repository a name.", "error");
  if (frMode === "clone" && !value) return frStatus("Paste the repository URL.", "error");

  $("fr-go").disabled = true;
  frStatus(frMode === "adopt" ? "Setting up" : "Setting up. This can take a moment.");
  try {
    await invoke("run_setup", { mode: frMode, value, location });
    $("firstrun").hidden = true;
    await showDone();
  } catch (e) {
    frStatus(String(e), "error");
  } finally {
    $("fr-go").disabled = false;
  }
});

// --- all set ---------------------------------------------------------------

async function showDone() {
  try {
    $("done-path").textContent = await invoke("data_dir");
  } catch {
    $("done-path").textContent = "your notes folder";
  }
  // Nothing to point at means nothing to offer; an inert checkbox is worse
  // than an absent one.
  const caps = await invoke("get_setup_state").then(s => s.capabilities).catch(() => null);
  if (!caps || !caps.app_installed) {
    $("done-app-row").hidden = true;
    $("done-app").checked = false;
  }
  $("done-desktop").textContent = caps ? `Shortcuts go in ${caps.desktop}` : "";
  $("done").hidden = false;
}

async function enterApp() {
  $("done").hidden = true;
  $("app").hidden = false;
  await loadSettings();
  await refresh(true);
}

$("done-go").addEventListener("click", async () => {
  const app = $("done-app").checked;
  const notes = $("done-notes").checked;
  $("done-go").disabled = true;
  try {
    if (app || notes) await invoke("create_shortcuts", { app, notes });
    await enterApp();
  } catch (e) {
    // A shortcut that could not be made is not a reason to trap someone on
    // this screen, say so, and let Continue take them through.
    const el = $("done-status");
    el.textContent = `${e}, continuing without it.`;
    el.dataset.kind = "error";
    el.hidden = false;
    $("done-go").textContent = "Continue anyway";
    $("done-go").onclick = enterApp;
  } finally {
    $("done-go").disabled = false;
  }
});

// --- actions ---------------------------------------------------------------

// The last message a failed refresh put on screen, so a later success can take
// it back down.
//
// Without this the first thing a new user sees is a red bar reading "no jotbay
// found at ~/jotbay", sitting above a dashboard that says "in sync, 3
// machines". It is written during setup. The app knows the vault path before
// the clone at that path exists, so the error is legitimate at the moment it is
// raised, and then nothing ever clears it. Pressing Sync happened to replace
// the text, which is why it looked intermittent.
//
// Functionally harmless, and the most alarming possible sentence for a tool
// holding somebody's notes to greet them with.
let staleStatusError = null;

async function refresh(fetchRemote) {
  try {
    state.status = await invoke("get_status", { refresh: fetchRemote });
    state.activity = await invoke("get_activity", { refresh: fetchRemote, limit: 60 });
    // Only this exact message, and only if it is still the one showing. sync()
    // sets its own toast and then calls us from a `finally`, so clearing
    // anything broader would eat the result the user just asked for, or worse,
    // hide a sync failure behind a status read that happened to succeed.
    if (staleStatusError && $("toast").textContent === staleStatusError) {
      toast(null);
    }
    staleStatusError = null;
    render();
  } catch (e) {
    staleStatusError = String(e);
    toast(staleStatusError, "error");
  }
}

async function sync() {
  if (state.syncing) return;
  state.syncing = true;
  toast("Syncing");
  render();

  try {
    const r = await invoke("do_sync");
    if (r.skipped_locked) {
      toast("Another sync is already running");
    } else if (!r.committed && r.pulled === 0 && !r.pushed && r.conflicts.length === 0) {
      toast("Already in sync");
    } else {
      const parts = [];
      if (r.committed) parts.push("committed");
      if (r.pulled) parts.push(`pulled ${r.pulled}`);
      if (r.conflicts.length) {
        parts.push(
          `${r.conflicts.length} ${plural(r.conflicts.length, "conflict")}. Both versions kept`
        );
      }
      if (r.pushed) parts.push("pushed");
      toast(parts.join(" · "));
    }
  } catch (e) {
    toast(String(e), "error");
  } finally {
    state.syncing = false;
    await refresh(false);
  }
}

async function loadSettings() {
  try {
    state.settings = await invoke("get_settings");
    $("set-theme").value = state.settings.theme;
    $("set-verbose").checked = state.settings.verbose;
    applyTheme();
  } catch (e) {
    toast(String(e), "error");
  }
}

async function saveSettings() {
  try {
    state.settings = await invoke("set_settings", {
      theme: $("set-theme").value,
      verbose: $("set-verbose").checked,
    });
    applyTheme();
    state.expanded.clear();
    render();
  } catch (e) {
    toast(String(e), "error");
  }
}

$("btn-upgrade").addEventListener("click", async () => {
  toast("Downloading update");
  try {
    const replaced = await invoke("do_upgrade");
    toast(`Updated ${replaced.join(", ")}, restart Jotbay to finish.`);
  } catch (e) {
    toast(String(e), "error");
  }
});

// The settings panel. Everything in it comes from one engine call, so the
// window, the macOS app and `jotbay about` cannot disagree about the facts.
function humanAge(secs) {
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

function renderAbout(a) {
  if (!a) return;
  state.about = a;
  $("ab-version").textContent = a.version;
  $("ab-machine").textContent = `${a.hostname}, ${a.os} ${a.arch}`;
  $("ab-notes").textContent = a.notes;
  $("ab-files").textContent = a.files;
  $("ab-branch").textContent = a.branch;
  // Already stripped of any credentials by the engine, because a settings
  // panel is exactly the sort of thing people screenshot into a bug report.
  $("ab-remote").textContent = a.remote || "none";
  $("ab-repo").textContent = a.tool_repo;
  $("ab-config").textContent = a.config_path;

  $("ab-sched").textContent = a.sync.scheduled ? "Installed" : "Not installed";
  $("ab-report").textContent =
    a.sync.last_report_secs === null ? "never" : humanAge(a.sync.last_report_secs);
  $("ab-running").textContent = a.sync.running_version || "-";

  // The one thing here that no other surface reports: replacing the binaries
  // does not restart the watcher, so this machine can be fully upgraded and
  // still sync with the old version, publishing that version as its own.
  const warn = $("ab-sync-warn");
  if (!a.sync.scheduled) {
    warn.textContent = "Nothing syncs in the background on this machine. Run jotbay schedule.";
    warn.hidden = false;
  } else if (a.sync.restart_needed) {
    warn.textContent =
      `The background sync is still running ${a.sync.running_version}. ` +
      `Restart it to pick up ${a.version}.`;
    warn.hidden = false;
  } else {
    warn.hidden = true;
  }
}

async function loadAbout() {
  try {
    renderAbout(await invoke("get_about"));
  } catch (e) {
    toast(String(e), "error");
  }
}

$("btn-check-updates").addEventListener("click", async () => {
  const line = $("ab-update");
  const btn = $("btn-check-updates");
  btn.disabled = true;
  line.hidden = false;
  line.textContent = "Checking";
  try {
    const a = await invoke("check_updates");
    renderAbout(a);
    line.textContent = a.update_available
      ? `Version ${a.update_available} is available.`
      : "This is the newest version.";
    line.className = a.update_available ? "warn" : "";
  } catch (e) {
    line.textContent = "Could not check.";
    line.className = "warn";
  }
  btn.disabled = false;
});

$("btn-open-notes").addEventListener("click", async () => {
  try {
    await invoke("open_data_dir");
  } catch (e) {
    toast(String(e), "error");
  }
});

$("btn-shortcuts").addEventListener("click", async () => {
  try {
    const made = await invoke("create_shortcuts", { app: true, notes: true });
    toast(made.length ? `Added ${made.join(", ")}` : "Nothing to add");
  } catch (e) {
    toast(String(e), "error");
  }
});

$("btn-settings").addEventListener("click", () => {
  const panel = $("settings-panel");
  panel.hidden = !panel.hidden;
  // Loaded on open rather than on every tick: none of it changes minute to
  // minute, and the panel is closed almost all of the time.
  if (!panel.hidden) loadAbout();
});
$("set-theme").addEventListener("change", saveSettings);
$("set-verbose").addEventListener("change", saveSettings);

// One button. Refresh used to sit beside it, which asked the user to know the
// difference between "fetch what others did" and "send what I did". A git
// distinction, in an app whose whole point is not needing one. Sync does both,
// and the background watcher means the window is current without being asked.
$("btn-sync").addEventListener("click", sync);
$("btn-folder").addEventListener("click", async () => {
  // Opening happens in Rust: plugin JS APIs are not guaranteed to be on the
  // global object under withGlobalTauri, only the core ones are.
  try {
    await invoke("open_data_dir");
  } catch (e) {
    toast(String(e), "error");
  }
});
$("btn-abort").addEventListener("click", async () => {
  try {
    await invoke("abort_rebase");
    toast("Rebase aborted");
    await refresh(false);
  } catch (e) {
    toast(String(e), "error");
  }
});

// Local-only repaint keeps ages honest; the network is only touched on an
// explicit refresh or sync.
(async () => {
  if (await showFirstRunIfNeeded()) return;
  $("app").hidden = false;
  await loadSettings();
  await refresh(true);
})();
setInterval(() => refresh(true), 20000);

// --- file browser ------------------------------------------------------------
//
// Read-only by construction: the backend exposes list and read, and nothing
// else. Markdown is rendered by the tiny renderer below rather than a library,
// because the CSP forbids remote scripts and a bundler is the one dependency
// this front end has managed to avoid.

let filesPath = "";

function fmtSize(b) {
  if (b >= 1048576) return (b / 1048576).toFixed(1) + " MB";
  if (b >= 1024) return Math.round(b / 1024) + " KB";
  return b + " B";
}
function fmtWhen(secs) {
  if (!secs) return "";
  const d = new Date(secs * 1000), diff = (Date.now() - d) / 1000;
  if (diff < 90) return "just now";
  if (diff < 3600) return Math.round(diff / 60) + "m ago";
  if (diff < 86400) return Math.round(diff / 3600) + "h ago";
  return d.toLocaleDateString();
}

/// Enough markdown for notes: headings, emphasis, code, lists, quotes, links.
/// Escapes first, then transforms. The input is the user's own notes, but a
/// note that happens to contain <script> must render as text, not run.
function renderMarkdown(src) {
  // The module-level esc(), not a local one: a first version shadowed it with
  // a three-character variant that skipped quotes, and the link rule below
  // interpolates into href="", where an unescaped quote in a synced note's
  // URL breaks out of the attribute. Notes sync from other machines, so their
  // content does not get to be trusted just because the owner is the reader.
  const inline = (t) =>
    t.replace(/`([^`]+)`/g, (_, c) => `<code>${c}</code>`)
     .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
     .replace(/\*([^*]+)\*/g, "<em>$1</em>")
     .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g,
              '<a href="$2" target="_blank" rel="noopener">$1</a>');

  // A pipe table row: | a | b |. The separator row that follows the header
  // (|---|:--:|) also matches, and is what proves a table rather than a line
  // that happens to contain pipes.
  const isRow = (t) => /^\s*\|.*\|\s*$/.test(t);
  const isSep = (t) => /^\s*\|(\s*:?-{2,}:?\s*\|)+\s*$/.test(t);
  const cells = (t) => t.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => c.trim());
  // Alignment comes from where the colons sit in the separator row.
  const align = (t) => cells(t).map((c) =>
    c.startsWith(":") && c.endsWith(":") ? "center" : c.endsWith(":") ? "right" : "left");

  const lines = esc(src).split("\n");
  let html = "", inCode = false, listTag = null, para = [];
  const closeList = () => { if (listTag) { html += `</${listTag}>`; listTag = null; } };
  const flushPara = () => {
    if (para.length) { html += `<p>${inline(para.join(" "))}</p>`; para = []; }
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (line.startsWith("```")) {
      flushPara(); closeList();
      html += inCode ? "</code></pre>" : "<pre><code>";
      inCode = !inCode;
      continue;
    }
    if (inCode) { html += line + "\n"; continue; }

    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) { flushPara(); closeList(); html += `<h${h[1].length}>${inline(h[2])}</h${h[1].length}>`; continue; }
    if (/^(-{3,}|\*{3,})\s*$/.test(line)) { flushPara(); closeList(); html += "<hr>"; continue; }

    // Tables, checked before lists: without this the pipes render as literal
    // text, which is what an 80-line reference document mostly turns into.
    if (!inCode && isRow(line) && i + 1 < lines.length && isSep(lines[i + 1])) {
      flushPara(); closeList();
      const cols = align(lines[i + 1]);
      const cell = (c, n, tag) =>
        `<${tag}${cols[n] && cols[n] !== "left" ? ` style="text-align:${cols[n]}"` : ""}>${inline(c)}</${tag}>`;
      html += "<table><thead><tr>" +
        cells(line).map((c, n) => cell(c, n, "th")).join("") + "</tr></thead><tbody>";
      i += 2;
      while (i < lines.length && isRow(lines[i])) {
        html += "<tr>" + cells(lines[i]).map((c, n) => cell(c, n, "td")).join("") + "</tr>";
        i++;
      }
      i--;
      html += "</tbody></table>";
      continue;
    }

    const ul = line.match(/^\s*[-*]\s+(.*)$/);
    const ol = line.match(/^\s*\d+[.)]\s+(.*)$/);
    if (ul || ol) {
      flushPara();
      const tag = ul ? "ul" : "ol";
      if (listTag !== tag) { closeList(); html += `<${tag}>`; listTag = tag; }
      const item = (ul || ol)[1];
      // `- [ ]` and `- [x]`: a checklist is a list whose whole point is the
      // boxes, and rendering them as literal brackets loses it.
      const task = item.match(/^\[([ xX])\]\s+(.*)$/);
      html += task
        ? `<li class="task"><input type="checkbox" disabled${task[1] === " " ? "" : " checked"}>` +
          `<span${task[1] === " " ? "" : ' class="done"'}>${inline(task[2])}</span></li>`
        : `<li>${inline(item)}</li>`;
      continue;
    }
    // Escaping runs before parsing, so the marker to match is &gt;, not >
    // matching > would never fire and quotes rendered as literal "&gt; text".
    const q = line.match(/^&gt;\s?(.*)$/);
    if (q) { flushPara(); closeList(); html += `<blockquote>${inline(q[1])}</blockquote>`; continue; }

    if (line.trim() === "") { flushPara(); closeList(); continue; }
    para.push(line.trim());
  }
  if (inCode) html += "</code></pre>";
  flushPara(); closeList();
  return html;
}

function crumbs() {
  const host = $("files-crumbs");
  const parts = filesPath ? filesPath.split("/") : [];
  let html = `<button data-path="">notes</button>`;
  let acc = "";
  parts.forEach((p, i) => {
    acc = acc ? `${acc}/${p}` : p;
    html += `<span class="sep">/</span>`;
    html += i === parts.length - 1
      ? `<span class="here">${esc(p)}</span>`
      : `<button data-path="${esc(acc)}">${esc(p)}</button>`;
  });
  host.innerHTML = html;
  host.querySelectorAll("button").forEach((b) =>
    b.addEventListener("click", () => openDir(b.dataset.path)));
}

async function openDir(path) {
  filesPath = path;
  $("file-preview").hidden = true;
  $("files-list").hidden = false;
  crumbs();
  let entries;
  try {
    entries = await invoke("list_notes", { path });
  } catch (e) {
    $("files-list").innerHTML = `<div class="frow"><span class="fname">${esc(String(e))}</span></div>`;
    return;
  }
  if (!entries.length) {
    $("files-list").innerHTML =
      `<div class="frow"><span class="fname" style="color:var(--text-faint)">Nothing here yet, files you put in this folder sync everywhere.</span></div>`;
    return;
  }
  $("files-list").innerHTML = entries.map((e) => `
    <div class="frow" data-path="${esc(e.path)}" data-dir="${e.is_dir}">
      <span class="ficon">${e.is_dir ? "&#128193;" : "&#128196;"}</span>
      <span class="fname">${esc(e.name)}</span>
      <span class="fmeta">${e.is_dir
        ? `${e.children} item${e.children === 1 ? "" : "s"}`
        : `${fmtSize(e.size)} &middot; ${fmtWhen(e.modified)}`}</span>
    </div>`).join("");
  $("files-list").querySelectorAll(".frow").forEach((row) =>
    row.addEventListener("click", () =>
      row.dataset.dir === "true" ? openDir(row.dataset.path) : openFile(row.dataset.path)));
}

async function openFile(path) {
  let file;
  try {
    file = await invoke("read_note", { path });
  } catch (e) { toast(String(e), "error"); return; }

  filesPath = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  filesPath = path;           // crumbs show the file itself; last part unclickable
  crumbs();
  filesPath = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";

  $("files-list").hidden = true;
  const meta = $("preview-meta");
  const body = $("preview-body");
  meta.innerHTML = `<span>${fmtSize(file.size)}</span>` +
    (file.truncated ? `<span>showing the first 2 MB</span>` : "");
  if (file.kind === "binary") {
    body.innerHTML = `<p style="color:var(--text-dim)">Not a text file, ${fmtSize(file.size)}. It syncs like everything else; open it from the folder.</p>`;
  } else if (file.kind === "markdown") {
    body.innerHTML = renderMarkdown(file.content);
  } else {
    body.innerHTML = `<pre>${esc(file.content)}</pre>`;
  }
  $("file-preview").hidden = false;
}

function showTab(which) {
  const files = which === "files";
  $("tab-activity").setAttribute("aria-selected", String(!files));
  $("tab-files").setAttribute("aria-selected", String(files));
  $("log").hidden = files;
  $("files").hidden = !files;
  $("log-count").hidden = files;
  if (files) openDir(filesPath);
}
$("tab-activity").addEventListener("click", () => showTab("activity"));
$("tab-files").addEventListener("click", () => showTab("files"));
