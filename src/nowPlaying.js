const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

const titleEl = document.querySelector("#title");
const metaEl = document.querySelector("#meta");
const artworkEl = document.querySelector("#artwork");

function render(d) {
  if (!d || !d.is_playing || !d.track_name) {
    titleEl.textContent = "Nothing is currently playing";
    metaEl.textContent = "";
    artworkEl.style.display = "none";
    artworkEl.removeAttribute("src");
    return;
  }
  titleEl.textContent = d.track_name;
  metaEl.textContent = [d.artists?.join(", "), d.album].filter(Boolean).join(" — ");

  if (d.artwork_url) {
    artworkEl.src = d.artwork_url;
    artworkEl.style.display = "block";
  } else {
    artworkEl.style.display = "none";
    artworkEl.removeAttribute("src");
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  // live updates
  await listen("now_playing_update", (evt) => render(evt.payload));

  // seed with current state once (in case song already playing)
  try {
    const d = await invoke("get_current_playing");
    render(d);
  } catch {}
});
