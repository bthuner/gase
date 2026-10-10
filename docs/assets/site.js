// The making of gase: small enhancements. The page reads fine without them.
(function () {
  "use strict";
  var doc = document.documentElement;
  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // The few words this script writes itself, in the page's language
  // (<html lang="fr"> for docs/fr/, English otherwise).
  var words = {
    en: {
      light: "Light", dark: "Dark",
      switchTo: function (t) { return "Switch to " + t + " theme"; },
      contents: "Contents",
      lz: function (n, l, r) { return "<b>" + n + "</b> characters became <b>" + l + "</b> literals and <b>" + r + "</b> back-references."; },
      lzTitle: function (len, dist) { return "copy " + len + " bytes from " + dist + " back"; }
    },
    fr: {
      light: "Clair", dark: "Sombre",
      switchTo: function (t) { return t === "light" ? "Passer au thème clair" : "Passer au thème sombre"; },
      contents: "Sommaire",
      lz: function (n, l, r) { return "<b>" + n + "</b> caractères sont devenus <b>" + l + "</b> littéraux et <b>" + r + "</b> références arrière."; },
      lzTitle: function (len, dist) { return "copier " + len + " octets depuis " + dist + " en arrière"; }
    }
  };
  var say = words[(doc.lang || "en").slice(0, 2)] || words.en;

  // ---------------------------------------------------------------- theme
  var toggle = document.querySelector("[data-theme-toggle]");
  function currentTheme() {
    if (doc.dataset.theme) return doc.dataset.theme;
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  function label() {
    if (!toggle) return;
    var next = currentTheme() === "dark" ? "light" : "dark";
    toggle.textContent = next === "light" ? say.light : say.dark;
    toggle.setAttribute("aria-label", say.switchTo(next));
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
      var title = say.contents;
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

  // LZ77 (chapter 15): greedy, matches of 3 to 258 bytes up to 32 KB back,
  // like DEFLATE's; overlapping copies allowed (distance < length).
  var lz = document.querySelector("[data-lz]");
  if (lz) {
    var input = lz.querySelector("input");
    var out = lz.querySelector("[data-lz-out]");
    var stats = lz.querySelector("[data-lz-stats]");
    var esc = function (t) { return t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/ /g, "\u00a0"); };
    var compress = function () {
      var t = input.value, i = 0, html = "", lits = 0, refs = 0;
      while (i < t.length) {
        var best = 0, dist = 0;
        for (var j = Math.max(0, i - 32768); j < i; j++) {
          var n = 0;
          while (n < 258 && i + n < t.length && t[j + n] === t[i + n]) n++;
          if (n > best) { best = n; dist = i - j; }
        }
        if (best >= 3) {
          html += '<span class="ref" title="' + say.lzTitle(best, dist) + '">' + esc(t.substr(i, best)) + "<small>" + dist + "</small></span>";
          i += best; refs++;
        } else {
          html += '<span class="lit">' + esc(t[i]) + "</span>";
          i++; lits++;
        }
      }
      out.innerHTML = html;
      stats.innerHTML = say.lz(t.length, lits, refs);
    };
    input.addEventListener("input", compress);
    compress();
  }
})();
