const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { WebviewWindow, getAll } = window.__TAURI__.webviewWindow;
const { open } = window.__TAURI__.dialog;

document.addEventListener("DOMContentLoaded", async () => {
  const statusEl = document.getElementById("status");
  const localDirEl = document.getElementById("local-dir");
  const nowPlayingEl = document.getElementById("now-playing"); // may be null
  const artworkEl = document.getElementById("artwork"); // may be null
  const exportToggle = document.getElementById("export-toggle");
  const out = await window.__TAURI__.core.invoke("debug_gsmtc");
  console.log("[GSMTC Debug]", out);

  const THEME_KEYS = {
    bg: "theme:bg",
    title: "theme:title",
    meta: "theme:meta",
  };

  let exportEnabled = JSON.parse(
    localStorage.getItem("exportEnabled") || "false"
  );
  if (exportToggle) exportToggle.checked = exportEnabled;
  let exportDir = null; // cached output directory
  let lastExportKey = ""; // de-dupe writes per track

  const showLocalDir = (dir) => {
    if (!localDirEl) return;
    localDirEl.textContent = dir
      ? `Local folder set: ${dir}`
      : "Art folder: (none set)";
  };

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
  function applyThemeToMain() {
    document.body.style.backgroundColor = "#2f2f2f";
  }

  async function broadcastTheme(theme) {
    try {
      await window.__TAURI__.event.emit("theme_update", theme);
    } catch {}
  }

  // Seed others (widget/settings) with current theme
  broadcastTheme(getTheme());

  // Seed main window & broadcast once on boot
  const bootTheme = getTheme();
  applyThemeToMain(bootTheme);
  broadcastTheme(bootTheme);

  // Respond when someone asks for the current theme (widget/settings)
  await listen("request_theme", async () => {
    await broadcastTheme(getTheme());
  });

  // Persist changes coming from the settings window, then broadcast
  await listen("theme_change", async (evt) => {
    const next = {
      bg: evt?.payload?.bg || "#2f2f2f",
      title: evt?.payload?.title || "#00cf00",
      meta: evt?.payload?.meta || "#ffffff",
    };
    setTheme(next);
    await broadcastTheme(next);
  });

  async function ensureExportDir() {
    if (exportDir) return exportDir;

    // Prefer the stored folder (you already save this via "Choose Music Folder")
    try {
      const saved = await invoke("get_local_art_dir");
      if (saved) {
        exportDir = saved;
        return exportDir;
      }
    } catch {}

    // If none is set, prompt once and store it
    const dir = await open({ directory: true, multiple: false });
    if (!dir) throw new Error("No export folder selected.");
    await invoke("set_local_art_dir", { path: dir });
    showLocalDir(dir);
    exportDir = dir;
    return exportDir;
  }

  function trackKey(d) {
    return [
      d?.track_name || "",
      (d?.artists || []).join(","),
      d?.album || "",
    ].join("||");
  }

  async function exportCurrent(d) {
    if (!d) return;
    const exportedTo = await invoke("write_now_playing_assets", {
      payload: {
        trackName: d.track_name || "",
        artists: d.artists || [],
        album: d.album || null,
        artworkUrl: d.artwork_url || null,
        artworkPath: d.artwork_path || null,
      },
    });
  }

  function maybeExport(d) {
    if (!exportEnabled || !d || !d.is_playing) return;
    const key = trackKey(d);
    if (key === lastExportKey) return; // same song => skip
    lastExportKey = key;
    exportCurrent(d).catch((err) =>
      setStatus("Export failed: " + err, "error")
    );
  }

  if (exportToggle) {
    exportToggle.addEventListener("change", async (e) => {
      exportEnabled = e.target.checked;
      localStorage.setItem("exportEnabled", JSON.stringify(exportEnabled));
      if (exportEnabled) {
        // Force an export immediately for the current track
        lastExportKey = "";
        try {
          const d = await invoke("get_current_playing");
          await maybeExport(d);
        } catch {}
      }
    });
  }

  // restore session
  try {
    const restored = await invoke("restore_spotify");
    if (restored) {
      setStatus("Connected!!!", "connected");
      console.log("Restore: ", restored);
    } else {
      setStatus("Not connected", "not-connected");
      console.log("Restore: ", restored);
    }
  } catch (e) {
    setStatus("Not connected", "not-connected");
    console.log("Restore Catch:", e);
  }

  // load and display the saved folder on startup
  try {
    const existingDir = await invoke("get_local_art_dir");
    showLocalDir(existingDir || null);
  } catch {
    showLocalDir(null);
  }

  function setStatus(text, type) {
    statusEl.textContent = text;
    statusEl.className = ""; // clear previous classes
    if (type) statusEl.classList.add(type);
  }

  function renderNowPlaying(d) {
    if (!artworkEl) return;

    // reset visuals
    artworkEl.style.display = "none";
    artworkEl.removeAttribute("src");
    if (nowPlayingEl) nowPlayingEl.textContent = "";

    // nothing playing
    if (!d || !d.is_playing || (!d.artwork_url && !d.artwork_path)) {
      if (nowPlayingEl)
        nowPlayingEl.textContent = "Nothing is currently playing.";
      return;
    }

    // show text
    if (d.track_name && nowPlayingEl) {
      const artists = (d.artists || []).join(", ");
      const album = d.album ? ` — ${d.album}` : "";
      nowPlayingEl.textContent = `▶ ${d.track_name}${
        artists ? " — " + artists : ""
      }${album}`;
    }

    // show art
    if (d.artwork_url) artworkEl.src = d.artwork_url;
    else if (d.artwork_path) artworkEl.src = convertFileSrc(d.artwork_path);
    artworkEl.style.display = "block";

    maybeExport(d);
  }

  // events
  await listen("now_playing_update", async (evt) => {
    const d = evt.payload;
    renderNowPlaying(d);
    await maybeExport(d); // <-- auto export on every change while checked
  });
  await listen("auth_lost", async () => {
    try {
      const ok = await invoke("restore_spotify");
      if (ok) {
        setStatus("Connected!!!", "connected");
        return;
      }
    } catch {}
    setStatus("Not connected", "not-connected");
    if (nowPlayingEl)
      nowPlayingEl.textContent = "Nothing is currently playing.";
    if (artworkEl) artworkEl.style.display = "none";
  });

  // choose local folder (requires tauri-plugin-dialog on the Rust side)
  const chooseBtn = document.getElementById("choose-folder");
  if (chooseBtn) {
    chooseBtn.addEventListener("click", async () => {
      const dir = await open({ directory: true, multiple: false });
      if (dir) {
        await invoke("set_local_art_dir", { path: dir });
        showLocalDir(dir);
        // Refresh now playing so the new folder is used right away
        try {
          const d = await invoke("get_current_playing");
          renderNowPlaying?.(d); // in main window
          // In the widget, the poll + event will also update, but you can do the same call there if needed
        } catch {}
      }
    });
  }

  // connect button
  const form = document.getElementById("connect-form");
  if (form) {
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      setStatus("Opening Spotify login...", "not-connected");
      try {
        await invoke("connect_spotify");
        setStatus("Connected!!!", "connected");
      } catch (err) {
        setStatus("Connect failed: " + err, "error");
      }
    });
  }

  // refresh button
  const refresh = document.getElementById("refresh-btn");
  if (refresh) {
    refresh.addEventListener("click", async () => {
      try {
        const d = await invoke("get_current_playing");
        renderNowPlaying(d);
      } catch (err) {
        if (nowPlayingEl)
          nowPlayingEl.textContent = "Unable to fetch now playing: " + err;
      }
    });
  }

  // --- widget window toggle: reliable close/open ---
  const openWidgetBtn = document.getElementById("open-widget");
  if (openWidgetBtn) {
    const WIDGET_LABEL_OPEN = "Open Widget Window";
    const WIDGET_LABEL_CLOSE = "Close Widget Window";

    // Strong reference to avoid quirks with lookups
    let widgetWin = null;

    async function findExistingWidget() {
      // If we already have a ref and it isn't destroyed, use it.
      if (widgetWin && typeof widgetWin.emit === "function") return widgetWin;

      try {
        if (typeof WebviewWindow.getByLabel === "function") {
          const maybe = WebviewWindow.getByLabel("widget");
          widgetWin = typeof maybe?.then === "function" ? await maybe : maybe;
        } else if (typeof getAll === "function") {
          const list = await getAll();
          widgetWin = (list || []).find((w) => w.label === "widget") || null;
        }
      } catch (e) {
        console.error("[widget] lookup failed:", e);
        widgetWin = null;
      }
      return widgetWin;
    }

    async function setWidgetBtnState() {
      const exists = !!(await findExistingWidget());
      openWidgetBtn.textContent = exists
        ? WIDGET_LABEL_CLOSE
        : WIDGET_LABEL_OPEN;
    }

    // init label (in case widget is already open)
    await setWidgetBtnState();

    async function openWidget() {
      const url = new URL("widget.html", window.location.href).toString();
      widgetWin = new WebviewWindow("widget", {
        url,
        title: "Widget",
        width: 380,
        height: 80,
        resizable: false,
        maximizable: false,
        decorations: false,
        transparent: true,
      });

      widgetWin.once("tauri://created", async () => {
        await setWidgetBtnState();
        await broadcastTheme(getTheme());
        widgetWin.setFocus().catch(() => {});
      });
      widgetWin.once("tauri://destroyed", async () => {
        widgetWin = null;
        await setWidgetBtnState();
      });

      // Reflect "open" immediately
      await setWidgetBtnState();
    }

    async function closeWidget() {
      const w = await findExistingWidget();
      if (!w) return;

      try {
        if (typeof w.close === "function") {
          await w.close();
          return;
        }
      } catch (e) {
        console.warn("[widget] close() failed:", e);
      }

      try {
        if (typeof w.destroy === "function") {
          await w.destroy();
          return;
        }
      } catch (e) {
        console.error("[widget] destroy() failed:", e);
      }
    }

    openWidgetBtn.addEventListener("click", async () => {
      const exists = !!(await findExistingWidget());
      if (exists) {
        await closeWidget();
        await setWidgetBtnState();
      } else {
        await openWidget();
      }
    });

    // Keep label synced if something else opens/closes the widget
    const _syncInterval = setInterval(setWidgetBtnState, 1000);
    window.addEventListener("beforeunload", () => clearInterval(_syncInterval));
  }

  const openSettingsBtn = document.getElementById("open-settings");
  if (openSettingsBtn) {
    async function findSettings() {
      try {
        if (typeof WebviewWindow.getByLabel === "function") {
          const maybe = WebviewWindow.getByLabel("settings");
          return typeof maybe?.then === "function" ? await maybe : maybe;
        } else if (typeof getAll === "function") {
          const list = await getAll();
          return (list || []).find((w) => w.label === "settings") || null;
        }
      } catch (e) {
        console.error("[settings] lookup failed:", e);
      }
      return null;
    }

    openSettingsBtn.addEventListener("click", async () => {
      const existing = await findSettings();
      if (existing) {
        try {
          await existing.setFocus();
        } catch {}
        return;
      }

      const url = new URL("settings.html", window.location.href).toString();
      const win = new WebviewWindow("settings", {
        url,
        title: "Settings",
        width: 380,
        height: 220,
        resizable: false,
        maximizable: false,
        decorations: true,
        transparent: false,
      });

      win.once("tauri://created", () => {
        console.log("[settings] created");
      });
      win.once("tauri://error", (e) => {
        console.error("[settings] failed to open:", e);
      });
    });
  }
});
