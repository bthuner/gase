// The making of gase: small enhancements. The page reads fine without them.
(function () {
  "use strict";
  var doc = document.documentElement;
  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // ---------------------------------------------------------------- theme
  var toggle = document.querySelector("[data-theme-toggle]");
  function currentTheme() {
    if (doc.dataset.theme) return doc.dataset.theme;
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  function label() {
    if (!toggle) return;
    var next = currentTheme() === "dark" ? "light" : "dark";
    toggle.textContent = next === "light" ? "Light" : "Dark";
    toggle.setAttribute("aria-label", "Switch to " + next + " theme");
  }
  if (toggle) {
    label();
    toggle.addEventListener("click", function () {
      var next = currentTheme() === "dark" ? "light" : "dark";
      doc.dataset.theme = next;
      try { localStorage.setItem("gase-theme", next); } catch (e) {}
      label();
    });
  }

  // ---------------------------------------------------------------- reveals
  var reveals = document.querySelectorAll(".reveal");
  if (!reduce && "IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) { e.target.classList.add("is-in"); io.unobserve(e.target); }
      });
    }, { rootMargin: "0px 0px -8% 0px", threshold: 0.05 });
    reveals.forEach(function (el) { io.observe(el); });
  } else {
    reveals.forEach(function (el) { el.classList.add("is-in"); });
  }

  // ---------------------------------------------------------------- progress + current chapter
  var bar = document.querySelector(".progress");
  var where = document.querySelector("[data-where]");
  var chapters = Array.prototype.slice.call(document.querySelectorAll(".chapter[data-title]"));
  var ticking = false;
  function onScroll() {
    ticking = false;
    var max = doc.scrollHeight - window.innerHeight;
    if (bar) bar.style.setProperty("--p", max > 0 ? Math.min(1, window.scrollY / max).toFixed(4) : 0);
    if (where) {
      var title = "Contents";
      var line = window.innerHeight * 0.3;
      chapters.forEach(function (c) { if (c.getBoundingClientRect().top < line) title = c.dataset.title; });
      if (where.textContent !== title) where.textContent = title;
    }
  }
  window.addEventListener("scroll", function () {
    if (!ticking) { ticking = true; window.requestAnimationFrame(onScroll); }
  }, { passive: true });
  onScroll();

  // ---------------------------------------------------------------- machine diagram
  var parts = document.querySelector("[data-parts]");
  if (parts) {
    var chips = document.querySelectorAll(".machine .chip");
    var set = function (name, on) {
      chips.forEach(function (c) { c.classList.toggle("on", on && c.dataset.part === name); });
      parts.querySelectorAll("li").forEach(function (li) { li.classList.toggle("on", on && li.dataset.part === name); });
    };
    chips.forEach(function (c) {
      c.addEventListener("mouseenter", function () { set(c.dataset.part, true); });
      c.addEventListener("mouseleave", function () { set(c.dataset.part, false); });
      c.addEventListener("focus", function () { set(c.dataset.part, true); });
      c.addEventListener("blur", function () { set(c.dataset.part, false); });
      var go = function () {
        var li = document.getElementById("part-" + c.dataset.part);
        if (li) li.scrollIntoView({ block: "nearest", behavior: reduce ? "auto" : "smooth" });
      };
      c.addEventListener("click", go);
      c.addEventListener("keydown", function (e) { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); go(); } });
    });
    parts.querySelectorAll("li").forEach(function (li) {
      li.addEventListener("mouseenter", function () { set(li.dataset.part, true); });
      li.addEventListener("mouseleave", function () { set(li.dataset.part, false); });
    });
  }

  // ---------------------------------------------------------------- FM
  var fm = document.querySelector("[data-fm]");
  if (fm) {
    var ratio = document.getElementById("fm-ratio");
    var index = document.getElementById("fm-index");
    var cycles = document.getElementById("fm-cycles");
    var lanes = { m: 60, c: 170, o: 290 };
    var draw = function () {
      var r = parseFloat(ratio.value), I = parseFloat(index.value), n = parseInt(cycles.value, 10);
      document.getElementById("fm-ratio-out").textContent = r;
      document.getElementById("fm-index-out").textContent = I.toFixed(1);
      document.getElementById("fm-cycles-out").textContent = n;
      var steps = 600, amp = 40, d = { m: "", c: "", o: "" };
      for (var i = 0; i <= steps; i++) {
        var x = (i / steps) * 1000;
        var t = (i / steps) * n * 2 * Math.PI;
        var mod = Math.sin(r * t);
        var ys = { m: mod, c: Math.sin(t), o: Math.sin(t + I * mod) };
        for (var k in ys) d[k] += (i ? "L" : "M") + x.toFixed(1) + " " + (lanes[k] - ys[k] * amp).toFixed(1);
      }
      fm.querySelectorAll("[data-trace]").forEach(function (p) { p.setAttribute("d", d[p.dataset.trace]); });
    };
    [ratio, index, cycles].forEach(function (el) { el.addEventListener("input", draw); });
    draw();
  }
})();
