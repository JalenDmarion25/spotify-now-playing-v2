console.log("[widget] script loaded");

const titleEl = document.querySelector("#title");
const metaEl = document.querySelector("#meta");
const artworkEl = document.querySelector("#artwork");
const widgetBody = document.querySelector("#widget-body");
const loopMeasure = new WeakMap();

let lastKey = "";

// --- THEME (local preview only; actual source of truth is main window) ---
const THEME_KEYS = { bg: "theme:bg", title: "theme:title", meta: "theme:meta" };

function getTheme() {
  return {
    bg: localStorage.getItem(THEME_KEYS.bg) || "#2f2f2f",
    title: localStorage.getItem(THEME_KEYS.title) || "#00cf00",
    meta: localStorage.getItem(THEME_KEYS.meta) || "#ffffff",
  };
}
function setTheme(theme) {
  localStorage.setItem(THEME_KEYS.bg, theme.bg);
  localStorage.setItem(THEME_KEYS.title, theme.title);
  localStorage.setItem(THEME_KEYS.meta, theme.meta);
}
function applyTheme(theme) {
  if (!theme) return;

  const bg = theme.bg ?? "#2f2f2f";
  if (widgetBody) widgetBody.style.backgroundColor = bg; // <-- background

  if (titleEl) titleEl.style.color = theme.title ?? "#00cf00";
  if (metaEl) metaEl.style.color = theme.meta ?? "#ffffff";
}

// --- ART/ANIM helpers (unchanged from your version) ---
function resolveArtUrl(d) {
  if (d?.artwork_url) return d.artwork_url;
  if (d?.artwork_path && window.__TAURI__?.core?.convertFileSrc) {
    return window.__TAURI__.core.convertFileSrc(d.artwork_path);
  }
  return "";
}
function ensureSpan(el) {
  let span = el.querySelector("span");
  if (!span) {
    span = document.createElement("span");
    el.textContent = "";
    el.appendChild(span);
  }
  return span;
}
function applyMarquee(el) {
  const span = ensureSpan(el);
  span.classList.remove("marquee");
  span.style.removeProperty("--marquee-start");
  span.style.removeProperty("--marquee-end");
  span.style.removeProperty("--marquee-duration");
  requestAnimationFrame(() => {
    const cs = getComputedStyle(span);
    const padR = parseFloat(cs.paddingRight) || 0;
    const boxW = el.clientWidth;
    const spanW = span.scrollWidth;
    const overflow = spanW - boxW;
    if (overflow <= 1) {
      span.style.transform = "translateX(0)";
      return;
    }
    const start = boxW;
    const end = spanW + padR;
    span.style.setProperty("--marquee-start", `${start}px`);
    span.style.setProperty("--marquee-end", `${end}px`);
    const distance = start + end;
    const pxPerSec = 60;
    const durationS = Math.max(8, Math.min(24, distance / pxPerSec));
    span.style.setProperty("--marquee-duration", `${durationS}s`);
    span.style.setProperty("--marquee-delay", "600ms");
    span.style.transform = `translateX(${start}px)`;
    void span.offsetWidth;
    span.classList.add("marquee");
  });
}
function showArtworkWithFade(url) {
  if (!url) {
    artworkEl.classList.remove("show");
    artworkEl.removeAttribute("src");
    return;
  }
  if (artworkEl.src === url) {
    artworkEl.classList.remove("show");
    void artworkEl.offsetWidth;
    artworkEl.classList.add("show");
    return;
  }
  artworkEl.classList.remove("show");
  artworkEl.onload = () => {
    artworkEl.onload = null;
    artworkEl.classList.add("show");
  };
  artworkEl.src = url;
}
const panelEl = document.getElementById("widget");
function slidePanel(show) {
  if (show) panelEl.classList.add("show");
  else panelEl.classList.remove("show");
}

function applyLoopIfOverflow(el) {
  const span = ensureSpan(el);
  requestAnimationFrame(() => {
    const boxW = el.clientWidth;
    const spanW = span.scrollWidth;
    const prev = loopMeasure.get(el);
    if (
      prev &&
      Math.abs(prev.boxW - boxW) < 1 &&
      Math.abs(prev.spanW - spanW) < 1 &&
      span.classList.contains("looping")
    ) {
      return;
    }
    loopMeasure.set(el, { boxW, spanW });
    if (spanW <= boxW + 1) {
      span.classList.remove("looping");
      span.style.transform = "translateX(0)";
      span.style.removeProperty("--loop-start");
      span.style.removeProperty("--loop-end");
      span.style.removeProperty("--loop-duration");
      return;
    }
    const START = 295,
      GAP = 24;
    span.style.setProperty("--loop-start", `${START}px`);
    span.style.setProperty("--loop-end", `${spanW + GAP}px`);
    const distance = START + spanW + GAP;
    const pxPerSec = 60;
    const durS = Math.max(12, Math.min(28, distance / pxPerSec));
    span.style.setProperty("--loop-duration", `${durS}s`);
    span.style.transform = `translateX(${START}px)`;
    span.classList.add("looping");
  });
}
function swapTextLikeOld(el, text) {
  const span = ensureSpan(el);
  el.animate(
    [
      { opacity: 1, transform: "translateX(0)" },
      { opacity: 0, transform: "translateX(100px)" },
    ],
    { duration: 300, fill: "forwards" }
  )
    .finished.then(() => {
      span.textContent = text;
      return el.animate(
        [
          { opacity: 0, transform: "translateX(100px)" },
          { opacity: 1, transform: "translateX(0)" },
        ],
        { duration: 300, fill: "forwards" }
      ).finished;
    })
    .then(() => applyLoopIfOverflow(el));
}

function render(d) {
  titleEl.classList.add("text-in");
  metaEl.classList.add("text-in");

  if (!d || !d.is_playing || !d.track_name) {
    slidePanel(false);
    swapTextLikeOld(titleEl, "Nothing is currently playing");
    swapTextLikeOld(metaEl, "");
    showArtworkWithFade(null);
    lastKey = "";
    return;
  }

  slidePanel(true);
  const title = d.track_name;
  const meta = d.artists;
  const art = resolveArtUrl(d);
  const key = `${title}|${meta}|${art}`;

  if (key !== lastKey) {
    swapTextLikeOld(titleEl, title);
    swapTextLikeOld(metaEl, meta);
    showArtworkWithFade(art);
    lastKey = key;
  } else {
    ensureSpan(titleEl).textContent = title;
    ensureSpan(metaEl).textContent = meta;
    applyLoopIfOverflow(titleEl);
    applyLoopIfOverflow(metaEl);
    if (art) {
      if (artworkEl.src !== art) artworkEl.src = art;
      artworkEl.classList.add("show");
    } else {
      artworkEl.classList.remove("show");
      artworkEl.removeAttribute("src");
    }
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  const tauri = window.__TAURI__;
  if (!tauri) return console.error("[widget] __TAURI__ not found");

  const { event, core } = tauri;

  // Theme: receive updates & request the current theme when we open
  await event.listen("theme_update", (evt) => applyTheme(evt.payload));
  await event.emit("request_theme");

  // Now playing updates
  await event.listen("now_playing_update", (evt) => {
    console.log("[widget] update", evt.payload);
    render(evt.payload);
  });

  // Seed once
  try {
    const d = await core.invoke("get_current_playing");
    render(d);
  } catch (e) {
    console.error("[widget] seed failed", e);
  }
});
