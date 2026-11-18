// The light and dark switch, shared by every page of the Console template.
// The stored choice is applied by a small inline script in each <head> before first paint;
// this file only wires the buttons. With no choice stored, the page is dark.
(function () {
  var root = document.documentElement;
  var lightBtn = document.getElementById("lightBtn");
  var darkBtn = document.getElementById("darkBtn");
  if (!lightBtn || !darkBtn) return;

  function paint() {
    var dark = (root.getAttribute("data-theme") || "dark") === "dark";
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
  paint();
})();
