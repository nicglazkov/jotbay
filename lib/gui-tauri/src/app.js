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
              changes: [],
              expanded: new Set() };

/// "1 conflict" rather than "1 conflict(s)". Parenthesised plurals are the
/// tell of a string that was never read aloud.
function plural(n, word) { return n === 1 ? word : `${word}s`; }

const EVENT_GLYPH = { changed: "↕", conflict: "⚠", error: "✖", offline: "◌" };
// Distinct per kind. One pair of arrows for everything meant a synced note and
// a rejected push looked exactly alike at a glance.
const CHANGE_GLYPH = { updated: "✎", conflict: "⚠", offline: "◌", problem: "✖" };


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
  // Before last_error, which is also set when a machine cannot reach the
  // remote. A commute is not a fault, and colouring it red is how red stops
  // meaning anything.
  if (node.offline) return "offline";
  if (node.last_error) return "error";
  // Before ordinary staleness. A machine whose credentials failed cannot
  // publish that fact, because publishing needs those credentials, so silence
  // is the only signal it can send and a day of it means something is wrong.
  if (ageSeconds(node.last_sync) > 86400) return "missing";
  if (ageSeconds(node.last_sync) > SYNC_INTERVAL * 3) return "stale";
  if (node.head !== localHead) return node.behind_local ? "behind" : "diverged";
  return "healthy";
}

const HEALTH_LABEL = {
  healthy: "in sync",
  behind: "behind",
  diverged: "diverged",
  stale: "not answering",
  missing: "silent for over a day",
  offline: "offline",
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
  } else if (s.nodes.some((n) => nodeHealth(n, s.head) === "error")) {
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

  // Mentioned, never as a warning: an offline machine needs nothing from
  // anyone and returns on its own.
  // Silent machines are counted with the broken ones: from a person's side
  // they are the same request for attention.
  const missing = s.nodes.filter((n) => nodeHealth(n, s.head) === "missing").length;
  if (missing && health === "healthy") {
    health = "diverged";
    headline = `${missing} machine${missing === 1 ? "" : "s"} silent for over a day`;
  }
  const offline = s.nodes.filter((n) => nodeHealth(n, s.head) === "offline").length;
  if (offline && health !== "error") {
    headline += ` · ${offline} machine${offline === 1 ? "" : "s"} offline`;
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
    // Only offer the button that can work. A .deb install belongs to apt, and
    // `jotbay upgrade` refuses it with an instruction; a button that can only
    // produce that refusal reads as broken.
    // Always offered now: the engine drives apt or the installer where those
    // own the files, so there is no route where the button cannot work.
    $("update-how").textContent = "";
    $("btn-upgrade").hidden = false;
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

/// One line per change, however many machines reported it.
function renderChanges() {
  const host = $("log");
  const items = state.changes || [];
  $("log-count").textContent = items.length;

  if (!items.length) {
    host.innerHTML = `<div class="empty">
      <div class="empty-title">Nothing has happened yet</div>
      <div class="empty-detail">Syncs that change nothing aren't recorded,
        so this stays quiet until something moves.</div>
    </div>`;
    return;
  }

  host.innerHTML = items
    .map((c, i) => {
      const files = c.files || [];
      const expandable = files.length > 0 || (state.settings.verbose && c.detail);
      const open = state.expanded.has(i);
      // Persistence, not a tally: it says the condition is still going.
      const repeat = c.repeats > 1 ? `<span class="event-repeat">x${c.repeats}</span>` : "";

      // Never claim authorship this machine cannot know: a change it only
      // received has no author in the buffer.
      const who = [];
      if (c.origin) who.push(esc(c.origin));
      else if (c.machines.length === 1) who.push(esc(c.machines[0]));
      if (c.machines.length > 1) who.push(`on ${c.machines.length} machines`);

      let body = "";
      if (open) {
        if (files.length) {
          body += `<div class="event-files">${files
            .map((f) => `<div>${esc(f)}</div>`)
            .join("")}</div>`;
        }
        if (state.settings.verbose && c.detail) {
          body += `<div class="event-detail">${esc(c.detail)}</div>`;
        }
      }

      return `<div class="event">
        <span class="event-glyph" data-kind="${c.kind}">${CHANGE_GLYPH[c.kind] || "·"}</span>
        <div class="event-main">
          <div class="event-summary" data-kind="${c.kind}">${esc(c.summary)} ${repeat}</div>
          <div class="event-meta">
            <span class="who">${who.join(" \u00b7 ")}</span>
            <span class="when">${humanAge(ageSeconds(c.at))}</span>
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
      renderChanges();
    })
  );
}

function renderActivity() {
  if (!state.settings.raw_activity) return renderChanges();
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
  // Loaded up front, not only when the settings panel opens: the update banner
  // needs to know whether this install can be upgraded in place before it
  // decides which button to show.
  await loadAbout();
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
    // One of the two shapes, never both: the window should not hold a feed
    // nobody is looking at, and each is a separate round trip.
    if (state.settings.raw_activity) {
      state.activity = await invoke("get_activity", { refresh: fetchRemote, limit: 60 });
    } else {
      state.changes = await invoke("get_changes", { refresh: fetchRemote, limit: 60 });
    }
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
    $("set-raw-activity").checked = !!state.settings.raw_activity;
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
      rawActivity: $("set-raw-activity").checked,
    });
    // The feed is a different shape in each mode, so it must be refetched
    // rather than re-rendered from what is already held.
    await refresh(false);
    applyTheme();
    state.expanded.clear();
    render();
  } catch (e) {
    toast(String(e), "error");
  }
}

$("btn-upgrade").addEventListener("click", async () => {
  const btn = $("btn-upgrade");
  btn.disabled = true;
  toast("Updating");
  try {
    // One call, whatever this machine was installed with. The engine drives
    // apt or the installer where those own the files, and restarts the
    // background sync afterwards so the machine actually runs what it just
    // installed.
    const o = await invoke("do_upgrade");
    let msg = o.already_current
      ? `Already on ${o.version}.`
      : `Updated to ${o.version}.`;
    if (!o.already_current && !o.sync_restarted) {
      msg += " No background sync is set up here.";
    }
    if (o.restart_app) msg += " Restart Jotbay to use it.";
    toast(msg);
    await loadAbout();
    await refresh(false);
  } catch (e) {
    toast(String(e), "error");
  }
  btn.disabled = false;
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

$("btn-settings").addEventListener("click", async () => {
  try {
    await invoke("open_settings_window");
  } catch (e) {
    toast(String(e), "error");
  }
});

// Settings are changed in the other window, so this one has to notice. Cheap
// enough to re-read whenever the window is looked at again.
if (!SETTINGS_ONLY) {
  window.addEventListener("focus", () => {
    loadSettings().then(() => refresh(false));
  });
}
$("set-theme").addEventListener("change", saveSettings);
$("set-verbose").addEventListener("change", saveSettings);
$("set-raw-activity").addEventListener("change", saveSettings);

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

// Settings runs as its own window, loading this same page in a mode that shows
// only the panel. One copy of the settings code rather than a second page that
// would drift from this one.
const SETTINGS_ONLY = new URLSearchParams(location.search).get("view") === "settings";

// Local-only repaint keeps ages honest; the network is only touched on an
// explicit refresh or sync.
(async () => {
  if (SETTINGS_ONLY) {
    document.body.classList.add("settings-only");
    $("app").hidden = false;
    $("settings-panel").hidden = false;
    await loadSettings();
    await loadAbout();
    return;
  }
  if (await showFirstRunIfNeeded()) return;
  $("app").hidden = false;
  await loadSettings();
  // Before the first refresh, because that is what draws the update banner and
  // the banner has to know whether this install can be upgraded in place.
  await loadAbout();
  await refresh(true);
})();
if (!SETTINGS_ONLY) setInterval(() => refresh(true), 20000);

// --- file browser ------------------------------------------------------------
//
// Read-only by construction: the backend exposes list and read, and nothing
// else. Markdown is rendered by the tiny renderer below rather than a library,
// because the CSP forbids remote scripts and a bundler is the one dependency
// this front end has managed to avoid.

let filesPath = "";
// The note being previewed. filesPath cannot serve: openFile rewrites it twice
// to draw the breadcrumbs and lands on the parent directory.
let currentFile = null;

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
  currentFile = null;
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

  currentFile = path;

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

// Files is where the window opens, so the listing has to be fetched without
// anyone clicking the tab. The markup alone would show an empty pane.
showTab("files");


// --- notes over time ---------------------------------------------------------
//
// Search, history, undelete and quick capture. History and undelete share one
// dialog because they are one idea: versions of a note. Settings is a window
// because it is about the app; these are about the note in front of you.

function sheet(title, sub, bodyHtml, footHtml) {
  $("sheet-title").textContent = title;
  $("sheet-sub").textContent = sub || "";
  $("sheet-body").innerHTML = bodyHtml;
  const foot = $("sheet-foot");
  foot.innerHTML = footHtml || "";
  foot.hidden = !footHtml;
  const dlg = $("sheet");
  if (!dlg.open) dlg.showModal();
}
$("sheet-close").addEventListener("click", () => $("sheet").close());

function whenText(iso) {
  const d = new Date(iso);
  return isNaN(d) ? "" : d.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

// --- finding -----------------------------------------------------------------

let searchTimer = null;
$("files-search").addEventListener("input", (e) => {
  const q = e.target.value.trim();
  clearTimeout(searchTimer);
  if (!q) {
    $("search-results").hidden = true;
    $("files-crumbs").hidden = false;
    $("files-list").hidden = false;
    return;
  }
  // Debounced: each search is a process, and typing a word would otherwise
  // start one per keystroke.
  searchTimer = setTimeout(() => runSearch(q), 180);
});

async function runSearch(q) {
  let hits = [];
  try {
    hits = await invoke("search_notes", { query: q, limit: 40 });
  } catch (e) {
    toast(String(e), "error");
    return;
  }
  if ($("files-search").value.trim() !== q) return;

  $("files-crumbs").hidden = true;
  $("files-list").hidden = true;
  $("file-preview").hidden = true;
  const host = $("search-results");
  host.hidden = false;
  if (!hits.length) {
    host.innerHTML = `<div class="empty"><div class="empty-title">No matches</div>
      <div class="empty-detail">Only notes that have synced at least once can be searched by content.</div></div>`;
    return;
  }
  host.innerHTML = hits
    .map(
      (h, i) => `<button class="frow hit" data-i="${i}">
        <span class="fname">${h.name_match ? "◆" : "·"} ${esc(h.path)}</span>
        ${h.excerpt ? `<span class="fmeta hit-excerpt">${esc(h.excerpt)}</span>` : ""}
      </button>`
    )
    .join("");
  host.querySelectorAll(".hit").forEach((b) =>
    b.addEventListener("click", () => {
      const hit = hits[Number(b.dataset.i)];
      $("files-search").value = "";
      host.hidden = true;
      $("files-crumbs").hidden = false;
      $("files-list").hidden = false;
      openFileByPath(hit.path);
    })
  );
}

/// Open a note by path, which is what a search result is asking for.
async function openFileByPath(path) {
  const dir = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  await openDir(dir);
  await openFile(path);
}

// --- a note over time ---------------------------------------------------------

$("btn-history").addEventListener("click", async () => {
  if (!currentFile) return;
  let versions = [];
  try {
    versions = await invoke("note_history", { rel: currentFile, limit: 50 });
  } catch (e) {
    return toast(String(e), "error");
  }
  const rows = versions.length
    ? versions
        .map(
          (v, i) => `<div class="vrow">
            <div>
              <div class="vwhen">${whenText(v.at)}${i === 0 ? ' <span class="vtag">current</span>' : ""}${
                v.deleted ? ' <span class="vtag warn">deleted</span>' : ""
              }</div>
              <div class="vmeta">${esc([v.machine, v.short].filter(Boolean).join(" · "))}</div>
            </div>
            ${i > 0 ? `<button class="link vrestore" data-sha="${esc(v.sha)}">Restore</button>` : ""}
          </div>`
        )
        .join("")
    : `<div class="empty"><div class="empty-title">No history yet</div>
       <div class="empty-detail">This note has not synced, so there is nothing to go back to.</div></div>`;

  sheet(
    "History",
    currentFile,
    rows,
    versions.length
      ? "Restoring writes the old text back as an ordinary change. Nothing is rewritten, and the newer version stays in the history."
      : null
  );
  $("sheet-body")
    .querySelectorAll(".vrestore")
    .forEach((b) =>
      b.addEventListener("click", () => restoreNote(currentFile, b.dataset.sha))
    );
});

$("btn-deleted").addEventListener("click", async () => {
  let gone = [];
  try {
    gone = await invoke("deleted_notes", { limit: 100 });
  } catch (e) {
    return toast(String(e), "error");
  }
  const rows = gone.length
    ? gone
        .map(
          (d) => `<div class="vrow">
            <div>
              <div class="vwhen">${esc(d.path)}</div>
              <div class="vmeta">${esc([d.machine, whenText(d.at)].filter(Boolean).join(" · "))}</div>
            </div>
            <button class="link vrestore" data-path="${esc(d.path)}">Restore</button>
          </div>`
        )
        .join("")
    : `<div class="empty"><div class="empty-title">Nothing has been deleted</div>
       <div class="empty-detail">Notes removed on any machine would show up here.</div></div>`;

  sheet("Deleted notes", "Still in the history, and recoverable", rows, null);
  $("sheet-body")
    .querySelectorAll(".vrestore")
    .forEach((b) => b.addEventListener("click", () => restoreNote(b.dataset.path, null)));
});

async function restoreNote(rel, version) {
  try {
    await invoke("restore_note", { rel, version });
    $("sheet").close();
    toast(`Restored ${rel}. It will sync with the next change.`);
    await openDir(filesPath);
    await refresh(false);
  } catch (e) {
    toast(String(e), "error");
  }
}

// --- quick capture and editing ------------------------------------------------

$("btn-new-note").addEventListener("click", () => {
  sheet(
    "New note",
    null,
    `<div class="newnote">
       <input id="new-note-name" type="text" placeholder="Name" autocomplete="off">
       <p>Created in your notes folder, and opened in your editor. Without an
          extension it becomes a .md file.</p>
       <div class="newnote-actions"><button id="new-note-go" class="primary">Create</button></div>
     </div>`,
    null
  );
  const input = $("new-note-name");
  input.focus();
  const go = async () => {
    const name = input.value.trim();
    if (!name) return;
    try {
      await invoke("create_note", { name });
      $("sheet").close();
      // Straight into the editor: creating a note and then having to find it
      // is most of the friction this removes.
      await invoke("open_note", { rel: name.includes(".") ? name : name + ".md" });
      await openDir(filesPath);
      await refresh(false);
    } catch (e) {
      toast(String(e), "error");
    }
  };
  $("new-note-go").addEventListener("click", go);
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") go();
  });
});

$("btn-open-editor").addEventListener("click", async () => {
  if (!currentFile) return;
  try {
    await invoke("open_note", { rel: currentFile });
  } catch (e) {
    toast(String(e), "error");
  }
});
