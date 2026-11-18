// The light and dark switch, shared by every page of the Console template.
// The inline script in each <head> applies ?theme=, then a saved choice, then the system
// setting, before first paint. This file wires the buttons and, until the reader picks a
// theme, keeps following the system setting when it changes.
(function () {
  var KEY = "jotbay-theme";
  var root = document.documentElement;
  var lightBtn = document.getElementById("lightBtn");
  var darkBtn = document.getElementById("darkBtn");
  if (!lightBtn || !darkBtn) return;

  function paint() {
    var dark = root.getAttribute("data-theme") === "dark";
    lightBtn.setAttribute("aria-pressed", String(!dark));
    darkBtn.setAttribute("aria-pressed", String(dark));
  }

  function choose(theme) {
    root.setAttribute("data-theme", theme);
    try { localStorage.setItem(KEY, theme); } catch (e) {}
    paint();
  }

  function picked() {
    if (/theme=(light|dark)/.test(location.search)) return true;
    try { var s = localStorage.getItem(KEY); return s === "light" || s === "dark"; } catch (e) { return false; }
  }

  var mq = window.matchMedia && window.matchMedia("(prefers-color-scheme: light)");
  if (mq && mq.addEventListener) {
    mq.addEventListener("change", function (e) {
      if (picked()) return;
      root.setAttribute("data-theme", e.matches ? "light" : "dark");
      paint();
    });
  }

  lightBtn.addEventListener("click", function () { choose("light"); });
  darkBtn.addEventListener("click", function () { choose("dark"); });
  paint();
})();
