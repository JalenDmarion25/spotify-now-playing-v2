console.log('[widget] script loaded');

const titleEl = document.querySelector('#title');
const metaEl = document.querySelector('#meta');
const artworkEl = document.querySelector('#artwork');

function render(d) {
  if (!d || !d.is_playing || !d.track_name) {
    titleEl.textContent = 'Nothing is currently playing';
    metaEl.textContent = '';
    artworkEl.style.display = 'none';
    artworkEl.removeAttribute('src');
    return;
  }
  titleEl.textContent = d.track_name;
  metaEl.textContent = [d.artists?.join(', '), d.album].filter(Boolean).join(' - ');
  if (d.artwork_url) {
    artworkEl.src = d.artwork_url;
    artworkEl.style.display = 'block';
  } else {
    artworkEl.style.display = 'none';
    artworkEl.removeAttribute('src');
  }
}

window.addEventListener('DOMContentLoaded', async () => {
  const tauri = window.__TAURI__;
  if (!tauri) return console.error('[widget] __TAURI__ not found');

  const { event, core } = tauri;

  // live updates
  await event.listen('now_playing_update', (evt) => {
    console.log('[widget] update', evt.payload);
    render(evt.payload);
  });

  // seed on open (don’t wait for the next poll tick)
  try {
    const d = await core.invoke('get_current_playing');
    render(d);
  } catch (e) {
    console.error('[widget] seed failed', e);
  }
});