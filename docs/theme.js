// The light and dark toggle, shared by every page.
//
// The stored preference is applied by a small inline script in each <head>,
// before the first paint. This file only wires the buttons, because a
// deferred external file would run too late to prevent a flash.
(function () {
  var root = document.documentElement;
  var lightBtn = document.getElementById("lightBtn");
  var darkBtn = document.getElementById("darkBtn");
  if (!lightBtn || !darkBtn) return;

  function paint() {
    var set = root.getAttribute("data-theme");
    var dark = set ? set === "dark"
                   : window.matchMedia("(prefers-color-scheme: dark)").matches;
    lightBtn.setAttribute("aria-pressed", String(!dark));
    darkBtn.setAttribute("aria-pressed", String(dark));
  }

  function choose(theme) {
    root.setAttribute("data-theme", theme);
    try { localStorage.setItem("jotbay-theme", theme); } catch (e) {}
    paint();
  }

  lightBtn.addEventListener("click", function () { choose("light"); });
  darkBtn.addEventListener("click", function () { choose("dark"); });
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", paint);
  paint();
})();
