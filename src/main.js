const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;


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
  // live updates from Rust
  await listen("now_playing_update", (evt) => {
    renderNowPlaying(evt.payload);
  });

  // attempt silent restore on boot (this will also start the watcher in Rust)
  try {
    const restored = await invoke("restore_spotify");
    if (restored) {
      statusEl.textContent = "Connected ✅";
    } else {
      statusEl.textContent = "Not connected";
    }
  } catch {
    statusEl.textContent = "Not connected";
  }

  document.querySelector("#connect-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    statusEl.textContent = "Opening Spotify login...";
    try {
      await invoke("connect_spotify"); // watcher starts in Rust after connect
      statusEl.textContent = "Connected ✅";
    } catch (err) {
      statusEl.textContent = "Connect failed: " + err;
    }
  });

  // optional manual refresh (kept as a backup)
  document.querySelector("#refresh-btn").addEventListener("click", async () => {
    try {
      const d = await invoke("get_current_playing");
      renderNowPlaying(d);
    } catch (err) {
      nowPlayingEl.textContent = "Unable to fetch now playing: " + err;
    }
  });
});

document.querySelector("#open-mini").addEventListener("click", async () => {
  try {
    await invoke("open_now_playing");
  } catch (e) {
    console.error(e);
  }
});