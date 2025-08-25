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
  const artists = d.artists?.join(", ") ?? "";
  const album = d.album ? ` — ${d.album}` : "";
  nowPlayingEl.textContent = `▶ ${d.track_name} — ${artists}${album}`;

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

  try {
    const restored = await invoke("restore_spotify");
    statusEl.textContent = restored ? "Connected ✅" : "Not connected";
  } catch {
    statusEl.textContent = "Not connected";
  }

  document.querySelector("#connect-form").addEventListener("submit", async (e) => {
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

  // --- bind to your EXISTING button ---
  const openBtn = document.getElementById("open-extra");
  openBtn.addEventListener("click", async () => {
    // If it's already open, just focus it
    const existing = (getAll?.() || []).find(w => w.label === "extra");
    if (existing) {
      try { await existing.setFocus(); } catch {}
      return;
    }

    // Resolve a correct URL relative to current page (tauri://localhost/…)
    const url = new URL("extra.html", window.location.href).toString();

    const win = new WebviewWindow("extra", {
      url,
      title: "Extra Window",
      width: 400,
      height: 300,
      resizable: true,
    });

    win.once("tauri://created", () => console.log("Extra window created"));
    win.once("tauri://error", (e) => console.error("Create failed:", e));
  });
});
