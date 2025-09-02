const { event, webviewWindow } = window.__TAURI__ || {};

const bgInput = document.getElementById("bg-color");
const titleInput = document.getElementById("title-color");
const metaInput = document.getElementById("meta-color");
const resetBtn = document.getElementById("reset-theme");
const closeBtn = document.getElementById("close");

// Preview colors inside the settings window, too
function applyThemeLocal(theme) {
  // just preview the settings window background so users see the pick
  document.body.style.backgroundColor = theme.bg || "#2f2f2f";
}

function readInputs() {
  return {
    bg: bgInput?.value || "#2f2f2f",
    title: titleInput?.value || "#00cf00",
    meta: metaInput?.value || "#ffffff",
  };
}

function setInputs(theme) {
  if (bgInput) bgInput.value = theme.bg || "#2f2f2f";
  if (titleInput) titleInput.value = theme.title || "#00cf00";
  if (metaInput) metaInput.value = theme.meta || "#ffffff";
}

async function emitChange() {
  const next = readInputs();
  try {
    await event.emit("theme_change", next);
  } catch {}
}

window.addEventListener("DOMContentLoaded", async () => {
  // Live listen for theme updates (e.g., broadcast from main or reset elsewhere)
  await event.listen("theme_update", (evt) => setInputs(evt.payload || {}));
  await event.emit("request_theme");

  // Hook inputs
  bgInput?.addEventListener("input", emitChange);
  titleInput?.addEventListener("input", emitChange);
  metaInput?.addEventListener("input", emitChange);

  resetBtn?.addEventListener("click", async () => {
    const def = { bg: "#2f2f2f", title: "#00cf00", meta: "#ffffff" };
    setInputs(def);
    await emitChange();
  });

  closeBtn?.addEventListener("click", async () => {
    try {
      const me = await webviewWindow.getCurrent();
      await me.close();
    } catch {}
  });
});
