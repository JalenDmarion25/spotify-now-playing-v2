const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { WebviewWindow, getAll } = window.__TAURI__.webviewWindow;

const statusEl = document.querySelector("#status");
const nowPlayingEl = document.querySelector("#now-playing");
const artworkEl = document.querySelector("#artwork");

function renderNowPlaying(d) {
  if (!d || !d.is_playing || !d.track_name) {
    nowPlayingEl.textContent = "Nothing is currently playing.";
    artworkEl.style.display = "none";
    artworkEl.removeAttribute("src");
    return;
  }
  if (d.artwork_url) {
    artworkEl.src = d.artwork_url;
    artworkEl.style.display = "block";
  } else {
    artworkEl.style.display = "none";
    artworkEl.removeAttribute("src");
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  // --- your existing listeners ---
  await listen("now_playing_update", (evt) => renderNowPlaying(evt.payload));

  await listen("auth_lost", () => {
  statusEl.textContent = "Not connected";
  nowPlayingEl.textContent = "Nothing is currently playing.";
  artworkEl.style.display = "none";
});

  try {
    const restored = await invoke("restore_spotify");
    statusEl.textContent = restored ? "Connected ✅" : "Not connected";
  } catch {
    statusEl.textContent = "Not connected";
  }

  document
    .querySelector("#connect-form")
    .addEventListener("submit", async (e) => {
      e.preventDefault();
      statusEl.textContent = "Opening Spotify login...";
      try {
        await invoke("connect_spotify");
        statusEl.textContent = "Connected ✅";
      } catch (err) {
        statusEl.textContent = "Connect failed: " + err;
      }
    });

  document.querySelector("#refresh-btn").addEventListener("click", async () => {
    try {
      const d = await invoke("get_current_playing");
      renderNowPlaying(d);
    } catch (err) {
      nowPlayingEl.textContent = "Unable to fetch now playing: " + err;
    }
  });

  document.getElementById("open-widget").addEventListener("click", () => {
    const existing = (getAll?.() || []).find((w) => w.label === "widget");
    if (existing) return existing.setFocus().catch(() => {});
    const url = new URL("widget.html", window.location.href).toString();
    new WebviewWindow("widget", {
      url,
      title: "Widget",
      width: 380,
      height: 80,
      resizable: false,
      alwaysOnTop: true,
      decorations: false,
      
    });
  });
});
