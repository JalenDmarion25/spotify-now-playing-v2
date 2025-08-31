console.log("[widget] script loaded");

const titleEl = document.querySelector("#title");
const metaEl = document.querySelector("#meta");
const artworkEl = document.querySelector("#artwork");
const loopMeasure = new WeakMap();

let lastKey = ""; // remembers last track to avoid animating on no-op updates

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

  // wipe previous run
  span.classList.remove("marquee");
  span.style.removeProperty("--marquee-start");
  span.style.removeProperty("--marquee-end");
  span.style.removeProperty("--marquee-duration");

  // measure after layout
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

    // start just off the right edge, end fully off the left WITH the extra gap
    const start = boxW;
    const end = spanW + padR; // ← was just spanW

    span.style.setProperty("--marquee-start", `${start}px`);
    span.style.setProperty("--marquee-end", `${end}px`);

    const distance = start + end; // boxW + spanW + padR
    const pxPerSec = 60;
    const durationS = Math.max(8, Math.min(24, distance / pxPerSec));
    span.style.setProperty("--marquee-duration", `${durationS}s`);
    span.style.setProperty("--marquee-delay", "600ms");

    // pre-position to avoid the flash during delay
    span.style.transform = `translateX(${start}px)`;

    void span.offsetWidth;
    span.classList.add("marquee");
  });
}

function swapTextWithAnimation(el, text) {
  const span = ensureSpan(el);

  // fade/slide out, set text, fade/slide in on next frames
  el.classList.remove("text-in");
  requestAnimationFrame(() => {
    span.textContent = text;
    requestAnimationFrame(() => {
      el.classList.add("text-in");
      applyMarquee(el);
    });
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

function setLooping(el, enable) {
  const span = ensureSpan(el);
  span.classList.toggle("looping", !!enable);
}

function applyLoopIfOverflow(el) {
  const span = ensureSpan(el);

  // measure after layout tick
  requestAnimationFrame(() => {
    const boxW  = el.clientWidth;
    const spanW = span.scrollWidth;

    // If nothing changed and we're already looping, do nothing.
    const prev = loopMeasure.get(el);
    if (prev &&
        Math.abs(prev.boxW - boxW) < 1 &&
        Math.abs(prev.spanW - spanW) < 1 &&
        span.classList.contains('looping')) {
      return;
    }

    loopMeasure.set(el, { boxW, spanW });

    // only loop if it actually overflows
    if (spanW <= boxW + 1) {
      span.classList.remove('looping');
      span.style.transform = 'translateX(0)';
      span.style.removeProperty('--loop-start');
      span.style.removeProperty('--loop-end');
      span.style.removeProperty('--loop-duration');
      return;
    }

    // mimic the “start right, end fully left”
    const START = 295;  // adjust to your layout
    const GAP   = 24;

    span.style.setProperty('--loop-start', `${START}px`);
    span.style.setProperty('--loop-end',   `${spanW + GAP}px`);

    const distance = START + spanW + GAP;
    const pxPerSec = 60;
    const durS     = Math.max(12, Math.min(28, distance / pxPerSec));
    span.style.setProperty('--loop-duration', `${durS}s`);

    // pre-position (matches keyframe "from")
    span.style.transform = `translateX(${START}px)`;

    // (re)start only when needed
    span.classList.add('looping');
  });
}

// Swap text → fade/slide in (your function), then decide loop vs static
function swapTextLikeOld(el, text) {
  const span = ensureSpan(el);

  // slide OUT to the right
  el.animate(
    [
      { opacity: 1, transform: "translateX(0)" },
      { opacity: 0, transform: "translateX(100px)" } // ← was -100px
    ],
    { duration: 300, fill: "forwards" }
  )
    .finished.then(() => {
      span.textContent = text;

      // slide IN from the right
      return el.animate(
        [
          { opacity: 0, transform: "translateX(100px)" }, // ← was -100px
          { opacity: 1, transform: "translateX(0)" }
        ],
        { duration: 300, fill: "forwards" }
      ).finished;
    })
    .then(() => {
      // after visible, decide if we need the loop
      applyLoopIfOverflow(el);
    });
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
  // const meta = [d.artists?.join(", "), d.album].filter(Boolean).join(" - ");
  const art  = resolveArtUrl(d);   
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

  titleEl.classList.add("text-in");
  metaEl.classList.add("text-in");

  // live updates
  await event.listen("now_playing_update", (evt) => {
    console.log("[widget] update", evt.payload);
    render(evt.payload);
  });

  // seed on open (don’t wait for the next poll tick)
  try {
    const d = await core.invoke("get_current_playing");
    render(d);
  } catch (e) {
    console.error("[widget] seed failed", e);
  }
});
