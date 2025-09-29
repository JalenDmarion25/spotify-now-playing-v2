console.log("[widget] script loaded");

const titleEl = document.querySelector("#title");
const metaEl = document.querySelector("#meta");
const artworkEl = document.querySelector("#artwork");
const widgetBody = document.querySelector("#widget-body");
const loopMeasure = new WeakMap();

let lastKey = "";

// --- THEME (local preview only; actual source of truth is main window) ---
const THEME_KEYS = { bg: "theme:bg", title: "theme:title", meta: "theme:meta" };
const SOURCE_KEY = "source:mode";
const GSMTC_APP_KEY = "gsmtc:app"; // "spotify" | "apple" | "ytm"
let gsmtcAppFilter = localStorage.getItem(GSMTC_APP_KEY) || "spotify";
let sourceMode = "spotify"; // default
let gsmPollId = null;

function setSourceMode(next) {
  sourceMode = next === "gsmtc" ? "gsmtc" : "spotify";
  localStorage.setItem(SOURCE_KEY, sourceMode);
  restartStrategy();
}

function stopGSMTCPoll() {
  if (gsmPollId) {
    clearInterval(gsmPollId);
    gsmPollId = null;
  }
}

function startGSMTCPoll() {
  stopGSMTCPoll();
  const poll = async () => {
    try {
      const d = await window.__TAURI__.core.invoke("get_current_playing_gsmtc");

      // DEBUG: see the actual app id so we can tweak matching if needed
      if (d?.source_app_id) {
        console.log("[GSMTC] source_app_id:", d.source_app_id);
      }

      renderGSMTC(d);
    } catch (e) {
      console.warn(e);
    }
  };
  poll();
  gsmPollId = setInterval(poll, 2000);
}

let spotifyUnsub = null;
async function startSpotifyListener() {
  stopSpotifyListener();
  spotifyUnsub = await window.__TAURI__.event.listen(
    "now_playing_update",
    (evt) => {
      render(evt.payload);
    }
  );
}
function stopSpotifyListener() {
  if (typeof spotifyUnsub === "function") {
    spotifyUnsub();
    spotifyUnsub = null;
  }
}

function restartStrategy() {
  if (sourceMode === "gsmtc") {
    stopSpotifyListener();
    startGSMTCPoll();
  } else {
    stopGSMTCPoll();
    startSpotifyListener();
  }
}

function matchesSelectedApp(d) {
  const idRaw = d?.source_app_id || "";
  const id = idRaw.toLowerCase();

  // Heuristics for common AUMIDs on Windows:
  // Spotify (Store + desktop builds)
  const isSpotify =
    id.includes("spotify");

  // Apple Music candidates:
  const isApple =
    id.includes("applemusicwin11") ||
    id.includes("appleinc.applemusic") ||
    id.includes("applemusic") ||
    id.includes("appleinc.itunes") ||
    id.includes("itunes");

  // YouTube Music candidates:
  const isYouTubeMusic =
    id.includes("youtubemusic") ||
    id.includes("youtube_music") ||
    (id.includes("google") && id.includes("music") && id.includes("youtube"));

  // Optional lenient: catch some PWA launcher shapes
  // (This still requires the AUMID to mention YouTube Music somewhere,
  // so regular browser tabs won't be picked up.)
  const isYouTubeMusicPWAish =
    isYouTubeMusic ||
    ((id.includes("pwa") || id.includes("pwalauncher")) && id.includes("youtube"));

  switch (gsmtcAppFilter) {
    case "spotify":
      return isSpotify;
    case "apple":
      return isApple;
    case "ytm":
      return isYouTubeMusicPWAish;
    default:
      return false;
  }
}

function isSpotifyGSMTC(d) {
  return (d?.source_app_id || "").toLowerCase().includes("spotify");
}

function renderGSMTC(d) {
  // Only show the selected app
  if (!matchesSelectedApp(d)) {
    render({ is_playing: false });
    return;
  }

  const status = (d?.status || "").toLowerCase();
  const active =
    ["playing", "paused"].includes(status) || d?.position_ms != null;
  if (!active) {
    render({ is_playing: false });
    return;
  }

  const track_name = (d?.title || "").trim();
  const artists = (d?.artist || "").trim();
  const album = (d?.album || "").trim();

  render({
    is_playing: true,
    track_name,
    artists,
    album,
    artwork_url: null,
    artwork_path: d?.artwork_path || null,
  });
}

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
  const meta =
    typeof d.artists === "string" ? d.artists : (d.artists || []).join(", ");
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

  // Theme channel (unchanged)
  await event.listen("theme_update", (evt) => applyTheme(evt.payload));
  await event.emit("request_theme");

  // Source mode channel: keep in sync with main window
  await event.listen("source_mode_update", (evt) => {
    const mode = evt?.payload?.mode;
    if (mode) setSourceMode(mode);
  });

  // Ask main window what to use
  await event.emit("request_source_mode");

  await event.listen("gsmtc_app_filter_update", (evt) => {
    const v = evt?.payload?.value;
    if (v) {
      gsmtcAppFilter = v;
      localStorage.setItem(GSMTC_APP_KEY, v);
    }
  });

  // Ask main for the current filter when we load
  await event.emit("request_gsmtc_app_filter");

  // Fallback if we didn’t hear back quickly (rare):
  setTimeout(() => {
    if (!sourceMode)
      setSourceMode(localStorage.getItem(SOURCE_KEY) || "spotify");
  }, 500);

  // Seed a first frame for whichever source we end up on
  // We don’t know the mode yet, but we can attempt both safely:
  try {
    // Spotify seed (may fail if not connected)
    const d = await core.invoke("get_current_playing");
    if (d?.track_name || d?.is_playing) render(d);
  } catch {}

  try {
    // GSMTC seed (cheap, doesn’t interfere)
    const g = await core.invoke("get_current_playing_gsmtc");
    renderGSMTC(g);
  } catch {}

  // Start the strategy (it will be replaced once source_mode_update arrives)
  restartStrategy();
});
