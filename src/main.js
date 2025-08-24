const { invoke } = window.__TAURI__.core;

const statusEl = document.querySelector("#status");
const nowPlayingEl = document.querySelector("#now-playing");
const artworkEl = document.querySelector("#artwork");


window.addEventListener("DOMContentLoaded", () => {
  document
    .querySelector("#connect-form")
    .addEventListener("submit", async (e) => {
      e.preventDefault();
      statusEl.textContent = "Opening Spotify login...";
      try {
        await invoke("connect_spotify");
        statusEl.textContent = "Connected ✅";
        await refreshNowPlaying();
      } catch (err) {
        statusEl.textContent = "Connect failed: " + err;
      }
    });

  document
    .querySelector("#refresh-btn")
    .addEventListener("click", refreshNowPlaying);
});

async function refreshNowPlaying() {
  try {
    const d = await invoke("get_current_playing");
    if (!d.is_playing || !d.track_name) {
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
  } catch (err) {
    nowPlayingEl.textContent = "Unable to fetch now playing: " + err;
    artworkEl.style.display = "none";
    artworkEl.removeAttribute("src");
  }
}
