"use strict";
(() => {
  const system = window.matchMedia("(prefers-color-scheme: light)");
  function apply() {
    let preference = {};
    try { preference = JSON.parse(localStorage.getItem("layrs.appearance.v1") || "{}") || {}; } catch (_) { /* Same dark default as Studio. */ }
    document.documentElement.dataset.theme = preference.theme === "light" || (preference.theme === "system" && system.matches) ? "light" : "dark";
    document.documentElement.dataset.motion = preference.reduceMotion === true ? "reduced" : "";
  }
  apply();
  system.addEventListener("change", apply);
  window.addEventListener("storage", event => { if (event.key === "layrs.appearance.v1" || event.key === null) apply(); });
})();
