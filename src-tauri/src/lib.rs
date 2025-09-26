use lofty::picture::{Picture, PictureType};
use lofty::prelude::{Accessor, TaggedFileExt};
use lofty::probe::Probe;
use parking_lot::lock_api::Mutex;
use parking_lot::Mutex as PlMutex;
use rspotify::{
    clients::{BaseClient, OAuthClient},
    model::{Image, PlayableItem},
    scopes, AuthCodePkceSpotify, Config, Credentials, OAuth, Token,
};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex as StdMutex, OnceLock};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::{Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use url::Url;
use walkdir::WalkDir;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession
};

#[derive(Default)]
struct SpotifyStore {
    // Use Arc so we can clone a handle and drop the lock before we await (fixes the Send error)
    client: Option<Arc<AuthCodePkceSpotify>>,
    watch_started: bool,
    cancel: Option<CancellationToken>,

    local_art_dir: Option<PathBuf>,
    art_cache: HashMap<String, String>, // album-key -> cached-art path
    local_index: HashMap<String, PathBuf>,
}

type SharedStore = Arc<PlMutex<SpotifyStore>>;

#[derive(Serialize)]
struct NowPlaying {
    is_playing: bool,
    track_name: Option<String>,
    artists: Vec<String>,
    album: Option<String>,
    artwork_url: Option<String>,  // remote (Spotify) URL
    artwork_path: Option<String>, // local file path, frontend will convert via convertFileSrc
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPayload {
    track_name: String,
    artists: Vec<String>,
    album: Option<String>,
    artwork_url: Option<String>,
    artwork_path: Option<String>,
}

fn sanitize(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let bad = ['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\n', '\r'];
    trimmed
        .chars()
        .map(|c| if bad.contains(&c) { '_' } else { c })
        .collect()
}

fn build_local_index(dir: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();

    for entry in WalkDir::new(dir)
        .follow_links(true)
        .max_depth(20)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !entry.file_type().is_file() || !is_audio(path) {
            continue;
        }

        let tagged = match Probe::open(path).and_then(|p| p.read()) {
            Ok(t) => t,
            Err(_) => continue,
        };

        // Prefer primary tag, fall back to first available.
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
        let (title, artist, album) = if let Some(t) = tag {
            let title = t.title().map(|s| s.to_string()).unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string()
            });
            let artist = t
                .artist()
                .unwrap_or(std::borrow::Cow::Borrowed(""))
                .to_string();
            let album = t
                .album()
                .unwrap_or(std::borrow::Cow::Borrowed(""))
                .to_string();
            (title, artist, album)
        } else {
            let fallback = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            (fallback, String::new(), String::new())
        };

        if !title.is_empty() {
            if !artist.is_empty() {
                map.insert(key_title_artist(&title, &artist), path.to_path_buf());
            }
            if !album.is_empty() {
                map.insert(key_title_album(&title, &album), path.to_path_buf());
            }
        }
    }

    map
}

fn extract_embedded_art_to_cache(app: &tauri::AppHandle, audio: &Path) -> Option<PathBuf> {
    let tagged = Probe::open(audio).ok()?.read().ok()?;

    // Pick a picture: prefer front cover, then any
    let mut pic_opt: Option<&Picture> = None;
    if let Some(t) = tagged.primary_tag() {
        let pics = t.pictures();
        pic_opt = pics
            .iter()
            .find(|p| matches!(p.pic_type(), PictureType::CoverFront | PictureType::Other))
            .or_else(|| pics.first());
    }
    if pic_opt.is_none() {
        if let Some(t) = tagged.first_tag() {
            let pics = t.pictures();
            pic_opt = pics
                .iter()
                .find(|p| matches!(p.pic_type(), PictureType::CoverFront | PictureType::Other))
                .or_else(|| pics.first());
        }
    }
    let pic = pic_opt?;

    let bytes: &[u8] = pic.data().as_ref();

    // Decide extension by MIME
    let ext = match pic.mime_type().map(|m| m.as_str()) {
        Some("image/jpeg") | Some("image/jpg") => "jpg",
        Some("image/png") => "png",
        Some("image/webp") => "webp",
        _ => "jpg",
    };

    // Cache path under $APP/artcache/<sanitized audio path>.<ext>
    let cache_dir = app.path().app_local_data_dir().ok()?.join("artcache");
    let _ = fs::create_dir_all(&cache_dir);

    // Make a deterministic filename from the audio path
    let mut name = audio.to_string_lossy().to_string();
    name = name.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");

    let out_path = cache_dir.join(format!("{}.{}", name, ext));
    fs::write(&out_path, bytes).ok()?;

    Some(out_path)
}

fn build_now_playing_from_ctx(ctx: &rspotify::model::CurrentlyPlayingContext) -> NowPlaying {
    use rspotify::model::PlayableItem;

    let mut track_name = None;
    let mut artists = Vec::new();
    let mut album = None;
    let mut artwork_url = None;

    if let Some(item) = &ctx.item {
        match item {
            PlayableItem::Track(track) => {
                track_name = Some(track.name.clone());
                artists = track.artists.iter().map(|a| a.name.clone()).collect();
                album = Some(track.album.name.clone());
                artwork_url = pick_image_url(&track.album.images, 300);
            }
            PlayableItem::Episode(ep) => {
                track_name = Some(ep.name.clone());
                album = Some(ep.show.name.clone());
                artists = vec![ep.show.publisher.clone()];
                artwork_url = pick_image_url(&ep.images, 300);
            }
        }
    }

    NowPlaying {
        is_playing: ctx.is_playing,
        track_name,
        artists,
        album,
        artwork_url,
        artwork_path: None,
    }
}

fn settings_path(window: &tauri::Window) -> Result<PathBuf, String> {
    let dir = window
        .app_handle()
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("app_local_data_dir: {e}"))?
        .join("settings");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir: {e}"))?;
    Ok(dir.join("settings.json"))
}

fn save_local_art_dir(window: &tauri::Window, path: &Path) -> Result<(), String> {
    let p = settings_path(window)?;
    let json = serde_json::json!({ "local_art_dir": path.to_string_lossy() });
    fs::write(p, serde_json::to_vec(&json).unwrap()).map_err(|e| e.to_string())
}

fn load_local_art_dir(window: &tauri::Window) -> Option<PathBuf> {
    let p = settings_path(window).ok()?;
    let bytes = fs::read(p).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("local_art_dir")?.as_str().map(PathBuf::from)
}

fn settings_path_from_handle(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("app_local_data_dir: {e}"))?
        .join("settings");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir: {e}"))?;
    Ok(dir.join("settings.json"))
}

fn load_local_art_dir_from_handle(app: &tauri::AppHandle) -> Option<PathBuf> {
    let p = settings_path_from_handle(app).ok()?;
    let bytes = fs::read(p).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("local_art_dir")?.as_str().map(PathBuf::from)
}

fn start_watcher_if_needed(app: &tauri::AppHandle, state: &SharedStore) {
    // Take the client and mark watcher started without holding the lock across await.
    let (client, should_start) = {
        let mut guard = state.lock();
        let c = guard.client.clone();
        let should = c.is_some() && !guard.watch_started;
        if should {
            guard.watch_started = true;
        }
        (c, should)
    };

    if !should_start {
        return;
    }

    let app = app.clone();
    let client = client.unwrap(); // safe because should_start implies Some

    let token = CancellationToken::new();
    {
        let mut g = state.lock();
        g.cancel = Some(token.clone());
    }

    tauri::async_runtime::spawn(async move {
        use tokio::time::{sleep, Duration};
        let state_handle = app.state::<SharedStore>();

        loop {
            tokio::select! {
              _ = token.cancelled() => break,

              _ = async {
                // if refresh fails -> auth is gone: clear everything and stop
                if client.auto_reauth().await.is_err() {
                  let _ = app.emit("auth_lost", &());
                  let mut s = state_handle.lock();
                  s.client = None;
                  s.watch_started = false;
                  s.cancel = None;
                  return;
                }
                let app_handle = app.clone();


                match client.current_user_playing_item().await {
                  Ok(Some(ctx)) => {
                    let mut np = build_now_playing_from_ctx(&ctx);
                    maybe_set_local_artwork(&app_handle, &state_handle, &mut np, &ctx);
                    let _ = app.emit("now_playing_update", &np);
                  }
                  Ok(None) => {
                    let _ = app.emit("now_playing_update", &NowPlaying {
                      is_playing: false,
                      track_name: None,
                      artists: vec![],
                      album: None,
                      artwork_url: None,
                      artwork_path: None,

                    });
                  }
                    Err(e) => {
                        // Transient API error (rate limit, network, 5xx, device issues, etc.)
                        // Don't mark auth lost; just keep polling.
                        // Optionally: if you can inspect the HTTP status and it's a hard 401 and reauth fails,
                        // then treat as fatal.
                        eprintln!("[poll] now_playing error: {e}");
                        // Emit a benign "nothing playing" or skip emitting anything:
                        let _ = app.emit("now_playing_update", &NowPlaying {
                            is_playing: false,
                            track_name: None,
                            artists: vec![],
                            album: None,
                            artwork_url: None,
                            artwork_path: None,
                        });
                        // then fall through to the sleep and next loop iteration
                    }
                }

                sleep(Duration::from_secs(2)).await;
              } => {}
            }
        }
    });
}

fn pick_image_url(images: &[Image], target: u32) -> Option<String> {
    if images.is_empty() {
        return None;
    }
    images
        .iter()
        .min_by_key(|img| {
            let w = img.width.unwrap_or(0);
            w.abs_diff(target)
        })
        .map(|img| img.url.clone())
}

fn read_token_from_disk(window: &tauri::Window) -> Result<Option<Token>, String> {
    let path = token_cache_path(window)?;
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read(&path).map_err(|e| format!("read token file: {e}"))?;
    let token: Token =
        serde_json::from_slice(&data).map_err(|e| format!("parse token json: {e}"))?;
    Ok(Some(token))
}

fn write_token_to_disk(window: &tauri::Window, token: &Token) -> Result<(), String> {
    let path = token_cache_path(window)?;
    let data = serde_json::to_vec(token).map_err(|e| format!("serialize token: {e}"))?;
    fs::write(&path, data).map_err(|e| format!("write token file: {e}"))
}

// pick a stable cache file; make sure the folder exists
fn token_cache_path(window: &tauri::Window) -> Result<PathBuf, String> {
    let path = window
        .app_handle()
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("app_local_data_dir: {e}"))?
        .join("spotify")
        .join("token.json");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create cache dir: {e}"))?;
    }
    Ok(path)
}

fn build_spotify(window: &tauri::Window) -> Result<AuthCodePkceSpotify, String> {
    let client_id =
        std::env::var("SPOTIFY_CLIENT_ID").map_err(|_| "Missing SPOTIFY_CLIENT_ID".to_string())?;

    let creds = Credentials::new(&client_id, "");
    let oauth = OAuth {
        redirect_uri: "http://127.0.0.1:5173/callback".to_string(),
        scopes: scopes!("user-read-currently-playing", "user-read-playback-state"),
        ..Default::default()
    };
    let config = Config {
        token_cached: true,
        token_refreshing: true,
        cache_path: token_cache_path(window)?,
        ..Default::default()
    };

    Ok(AuthCodePkceSpotify::with_config(creds, oauth, config))
}

fn clear_token_cache(window: &tauri::Window) -> Result<(), String> {
    let path = token_cache_path(window)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("remove token file: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
async fn write_now_playing_assets(
    _window: tauri::Window,
    payload: ExportPayload,
) -> Result<String, String> {
    use std::fs;

    let exe_dir = std::env::current_exe()
        .map_err(|e| format!("current_exe: {e}"))?
        .parent()
        .ok_or_else(|| "Cannot resolve executable directory".to_string())?
        .to_path_buf();

    let dir = exe_dir.join("Exported-track");
    fs::create_dir_all(&dir).map_err(|e| format!("create Exported-track: {e}"))?;

    // --- write the text files ---
    let song = sanitize(&payload.track_name);
    let artists = sanitize(&payload.artists.join(", "));
    let album = sanitize(payload.album.as_deref().unwrap_or(""));

    fs::write(dir.join("song.txt"), song).map_err(|e| e.to_string())?;
    fs::write(dir.join("artist.txt"), artists).map_err(|e| e.to_string())?;
    fs::write(dir.join("album.txt"), album).map_err(|e| e.to_string())?;

    // --- artwork -> PNG (prefer local path, else fetch URL) ---
    let target = dir.join("artwork.png");

    if let Some(ap) = payload.artwork_path.as_deref() {
        if !ap.is_empty() && Path::new(ap).exists() {
            if let Ok(img) = image::open(ap) {
                img.save(&target).map_err(|e| e.to_string())?;
                return Ok(dir.to_string_lossy().to_string());
            }
            if Path::new(ap)
                .extension()
                .and_then(|e| e.to_str())
                .map_or(false, |x| x.eq_ignore_ascii_case("png"))
            {
                fs::copy(ap, &target).map_err(|e| e.to_string())?;
                return Ok(dir.to_string_lossy().to_string());
            }
        }
    }

    if let Some(url) = payload.artwork_url.as_deref() {
        if !url.is_empty() {
            let bytes = reqwest::get(url)
                .await
                .map_err(|e| e.to_string())?
                .bytes()
                .await
                .map_err(|e| e.to_string())?;
            let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
            img.save(&target).map_err(|e| e.to_string())?;
        }
    }

    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
fn set_local_art_dir(
    _state: State<'_, SharedStore>, // underscore to silence unused warning
    window: tauri::Window,
    path: String,
) -> Result<(), String> {
    let pb = PathBuf::from(path);
    if !pb.is_dir() {
        return Err("Not a directory".into());
    }
    save_local_art_dir(&window, &pb)?;

    let app = window.app_handle().clone(); // ← clone fixes E0597
    tauri::async_runtime::spawn_blocking(move || {
        let idx = build_local_index(&pb);
        let s = app.state::<SharedStore>();
        let mut g = s.lock();
        g.local_art_dir = Some(pb);
        g.art_cache.clear();
        g.local_index = idx;
    });

    Ok(())
}

#[tauri::command]
fn get_local_art_dir(state: State<'_, SharedStore>, window: tauri::Window) -> Option<String> {
    // prefer in-memory; else try disk
    let mem = state
        .lock()
        .local_art_dir
        .clone()
        .or_else(|| load_local_art_dir(&window));
    mem.map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
async fn restore_spotify(
    state: State<'_, SharedStore>,
    window: tauri::Window,
) -> Result<bool, String> {
    let spotify = build_spotify(&window)?;
    if let Some(token) = read_token_from_disk(&window)? {
        {
            let token_mutex = spotify.get_token();
            let mut guard = token_mutex
                .lock()
                .await
                .map_err(|_| "Token lock failed".to_string())?;
            *guard = Some(token);
        }

        // ⬇️ check the result; if it fails, clear cache and report false
        if let Err(_) = spotify.auto_reauth().await {
            let _ = clear_token_cache(&window);
            let mut s = state.lock();
            if let Some(t) = s.cancel.take() {
                t.cancel();
            }
            s.client = None;
            s.watch_started = false;
            return Ok(false);
        }

        if let Some(tok) = spotify
            .get_token()
            .lock()
            .await
            .map_err(|_| "Token lock failed".to_string())?
            .clone()
        {
            let _ = write_token_to_disk(&window, &tok);
        }
        state.lock().client = Some(Arc::new(spotify));

        let app = window.app_handle();
        start_watcher_if_needed(&app, &state);
        return Ok(true);
    }
    Ok(false)
}

static LAST_GSMTC_TRACK: OnceLock<StdMutex<Option<String>>> = OnceLock::new();

#[tauri::command]
async fn get_current_playing_gsmtc(window: tauri::Window) -> Result<serde_json::Value, String> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

    let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .map_err(|e| format!("RequestAsync failed: {:?}", e))?
        .await
        .map_err(|e| format!("Await manager failed: {:?}", e))?;

    // ---- pick the Spotify session if present
    let session: Option<GlobalSystemMediaTransportControlsSession> = match mgr.GetSessions() {
        Ok(list) => {
            let n = list.Size().unwrap_or(0);
            let mut picked = None;
            for i in 0..n {
                if let Ok(s) = list.GetAt(i) {
                    if let Ok(aumid) = s.SourceAppUserModelId() {
                        if aumid.to_string().to_ascii_lowercase().contains("spotify") {
                            picked = Some(s);
                            break;
                        }
                    }
                }
            }
            // fallback to current session if Spotify not found
            picked.or_else(|| mgr.GetCurrentSession().ok())
        }
        Err(_) => mgr.GetCurrentSession().ok(),
    };

    let Some(session) = session else {
        return Ok(serde_json::json!({"error": "No active session"}));
    };

    // status
    let status = session
        .GetPlaybackInfo()
        .ok()
        .and_then(|info| info.PlaybackStatus().ok())
        .map(|s| format!("{:?}", s))
        .unwrap_or_else(|| "Unknown".to_string());

    // media props
    let props = session
        .TryGetMediaPropertiesAsync()
        .map_err(|e| format!("TryGetMediaPropertiesAsync: {:?}", e))?
        .await
        .map_err(|e| format!("await media properties: {:?}", e))?;

    let title = props.Title().unwrap_or_default().to_string();
    let album = props.AlbumTitle().unwrap_or_default().to_string();
    let artist = props.Artist().unwrap_or_default().to_string();

    // timeline (safe to read synchronously)
    let (position_ms, end_time_ms, last_updated_iso) = match session.GetTimelineProperties() {
        Ok(tl) => {
            let pos_ms = tl.Position().ok().map(|ts| ts.Duration / 10_000);
            let end_ms = tl.EndTime().ok().map(|ts| ts.Duration / 10_000);
            let last_updated = tl.LastUpdatedTime().ok().map(|dt| format!("{:?}", dt));
            (pos_ms, end_ms, last_updated)
        }
        Err(_) => (None, None, None),
    };

    let payload = serde_json::json!({
        "status": status,
        "title": title,
        "album": album,
        "artist": artist,
        "position_ms": position_ms,
        "end_time_ms": end_time_ms,
        "last_updated": last_updated_iso,
        "source_app_id": session.SourceAppUserModelId().ok().map(|s| s.to_string())
    });

    // ---- change detection + emit
    let key = format!("{}|{}|{}", title, artist, album);

    // get or init a std::sync::Mutex so lock() -> Result<..>
    let cell = LAST_GSMTC_TRACK.get_or_init(|| StdMutex::new(None));
    let mut guard = cell.lock().unwrap();

    // compare without as_deref to avoid type mismatches
    let is_new = match &*guard {
        Some(prev) => prev != &key,
        None => true,
    };

    if is_new {
        *guard = Some(key.clone()); // store a clone
        let _ = window.emit("gsmtc_track_changed", &payload);
    }

    println!("[DEBUG GSMTC] {payload}");
    Ok(payload)
}

#[tauri::command]
async fn connect_spotify(
    state: State<'_, SharedStore>,
    window: tauri::Window,
) -> Result<(), String> {
    // 0) If we already have a client, just refresh and return (no browser)
    let existing = {
        let guard = state.lock(); // guard lives only inside this block
        guard.client.clone()
    }; // guard dropped here BEFORE the await below

    if let Some(existing) = existing {
        let _ = existing.auto_reauth().await; // now this future is Send
        return Ok(());
    }

    // 1) Build client + stable cache path
    let client_id =
        std::env::var("SPOTIFY_CLIENT_ID").map_err(|_| "Missing SPOTIFY_CLIENT_ID".to_string())?;
    let redirect_uri = "http://127.0.0.1:5173/callback".to_string();

    let cache_path = token_cache_path(&window)?;
    let creds = Credentials::new(&client_id, "");
    let oauth = OAuth {
        redirect_uri: redirect_uri.clone(),
        scopes: scopes!("user-read-currently-playing", "user-read-playback-state"),
        ..Default::default()
    };
    let config = Config {
        token_cached: true,
        token_refreshing: true,
        cache_path,
        ..Default::default()
    };
    let mut spotify = AuthCodePkceSpotify::with_config(creds, oauth, config);

    // 2) Try to reuse a cached token (no browser)
    let _ = spotify.read_token_cache(true).await; // load even if expired; we'll refresh

    let token_mutex = spotify.get_token();
    let has_cached = {
        let guard = token_mutex
            .lock()
            .await
            .map_err(|_| "Token lock failed".to_string())?;
        guard.is_some()
    };

    if has_cached {
        let _ = spotify.auto_reauth().await; // refresh if needed
        let _ = spotify.write_token_cache().await; // persist any new token
        state.lock().client = Some(Arc::new(spotify));
        return Ok(());
    }

    // 3) First-time auth: open browser, wait for code, exchange, cache, store
    let auth_url = spotify.get_authorize_url(None).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_url(auth_url.as_str(), None::<&str>).map_err(|e| e.to_string())?;

    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let addr = "127.0.0.1:5173".to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = run_callback_server_blocking(&addr, tx);
    });

    let code = rx.await.map_err(|e| format!("Callback wait error: {e}"))?;
    spotify
        .request_token(&code)
        .await
        .map_err(|e| format!("Token exchange failed: {e}"))?;

    // Persist the token we just received
    if let Some(tok) = spotify
        .get_token()
        .lock()
        .await
        .map_err(|_| "Token lock failed".to_string())?
        .clone()
    {
        let _ = write_token_to_disk(&window, &tok);
    }

    state.lock().client = Some(Arc::new(spotify));

    let app = window.app_handle();
    start_watcher_if_needed(&app, &state);

    Ok(())
}

// Minimal HTTP server just for the OAuth redirect
fn run_callback_server_blocking(
    addr: &str,
    tx: tokio::sync::oneshot::Sender<String>,
) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("Bind {addr} failed: {e}"))?;

    // Accept exactly one request that contains /callback?code=...
    for stream in listener.incoming() {
        let mut stream = stream.map_err(|e| format!("Accept failed: {e}"))?;

        // Read the HTTP request (first packet is enough for our tiny case)
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);

        // Parse Request-Line: e.g. "GET /callback?code=... HTTP/1.1"
        let first_line = req.lines().next().unwrap_or("");
        // Find the path segment between "GET " and " HTTP"
        let path = if first_line.starts_with("GET ") {
            let start = 4;
            if let Some(end) = first_line[start..].find(" HTTP") {
                &first_line[start..start + end]
            } else {
                "/"
            }
        } else {
            "/"
        };

        // Build a full URL so we can use `url` parser
        let full = format!("http://localhost{path}");
        if let Ok(parsed) = Url::parse(&full) {
            if parsed.path() == "/callback" {
                if let Some(code) = parsed.query_pairs().find_map(|(k, v)| {
                    if k == "code" {
                        Some(v.to_string())
                    } else {
                        None
                    }
                }) {
                    // Respond to the browser
                    let body = "You can close this tab and return to the app. ✅";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());

                    // Deliver the code back to the app and stop
                    let _ = tx.send(code);
                    break;
                }
            }
        }

        // Fallback 404 if not the expected route
        let body = "Not Found";
        let resp = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(resp.as_bytes());
    }

    Ok(())
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

fn key_title_artist(title: &str, artist: &str) -> String {
    format!("{}|{}", norm(title), norm(artist))
}
fn key_title_album(title: &str, album: &str) -> String {
    format!("{}|{}", norm(title), norm(album))
}

fn is_audio(p: &Path) -> bool {
    match p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
    {
        Some(ref e)
            if [
                "aa", "aax", "aac", "aiff", "ape", "dsf", "flac", "m4a", "m4b", "m4p", "mp3",
                "mpc", "mpp", "ogg", "oga", "wav", "wma", "wv", "webm",
            ]
            .contains(&e.as_str()) =>
        {
            true
        }
        _ => false,
    }
}

fn try_common_names(dir: &Path) -> Option<PathBuf> {
    const NAMES: &[&str] = &[
        "cover.jpg",
        "cover.png",
        "folder.jpg",
        "folder.png",
        "front.jpg",
        "front.png",
        "album.jpg",
        "album.png",
        "art.jpg",
        "art.png",
    ];
    for n in NAMES {
        let p = dir.join(n);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn find_local_art_in_base(
    base: &Path,
    artist: &str,
    album: Option<&str>,
    track: &str,
) -> Option<PathBuf> {
    let a_norm = norm(artist);
    let alb_norm = album.as_ref().map(|s| norm(s));
    let t_norm = norm(track);

    // 0) quick sanity
    if !base.is_dir() {
        return None;
    }

    // 1) Prefer directories that look like the album/artist/track and check common names there.
    //    Go a bit deeper to handle things like "(Mixtapes)/Burn After Rolling".
    for entry in walkdir::WalkDir::new(base)
        .follow_links(true)
        .max_depth(8)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_dir())
    {
        let name = entry.path().file_name().and_then(|n| n.to_str()).map(norm);

        if let Some(n) = name {
            let looks_like_album = alb_norm
                .as_deref()
                .map(|alb| n.contains(alb))
                .unwrap_or(false);
            let looks_like_artist = n.contains(&a_norm);
            let looks_like_track = n.contains(&t_norm);

            if looks_like_album || looks_like_artist || looks_like_track {
                if let Some(p) = try_common_names(entry.path()) {
                    return Some(p);
                }
            }
        }
    }

    // 2) Broader file scan (still bounded). Accept if the parent OR grandparent looks like album/artist/track,
    //    or if the filename itself looks like it.
    for entry in walkdir::WalkDir::new(base)
        .follow_links(true)
        .max_depth(8)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let ext_ok = matches!(
            entry.path().extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()),
            Some(ref e) if ["jpg","jpeg","png","webp"].contains(&e.as_str())
        );
        if !ext_ok {
            continue;
        }

        // parent dir
        let parent_norm = entry
            .path()
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(norm)
            .unwrap_or_default();

        // grandparent dir (optional)
        let gp_norm = entry
            .path()
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(norm)
            .unwrap_or_default();

        // filename (without extension)
        let stem_norm = entry
            .path()
            .file_stem()
            .and_then(|n| n.to_str())
            .map(norm)
            .unwrap_or_default();

        let matches_dirs = parent_norm.contains(&a_norm)
            || parent_norm.contains(&t_norm)
            || alb_norm
                .as_deref()
                .map(|alb| parent_norm.contains(alb))
                .unwrap_or(false)
            || gp_norm.contains(&a_norm)
            || gp_norm.contains(&t_norm)
            || alb_norm
                .as_deref()
                .map(|alb| gp_norm.contains(alb))
                .unwrap_or(false);

        let matches_name = stem_norm.contains(&a_norm)
            || stem_norm.contains(&t_norm)
            || alb_norm
                .as_deref()
                .map(|alb| stem_norm.contains(alb))
                .unwrap_or(false);

        if matches_dirs || matches_name {
            return Some(entry.path().to_path_buf());
        }
    }

    None
}

fn maybe_set_local_artwork(
    app: &tauri::AppHandle,
    state: &SharedStore,
    np: &mut NowPlaying,
    ctx: &rspotify::model::CurrentlyPlayingContext,
) {
    // Already has Spotify art?
    if np.artwork_url.is_some() {
        return;
    }

    let (artist, album, track, _is_local) = match &ctx.item {
        Some(PlayableItem::Track(t)) => {
            let first_artist = t.artists.get(0).map(|a| a.name.as_str()).unwrap_or("");
            (
                first_artist.to_string(),
                Some(t.album.name.clone()),
                t.name.clone(),
                t.is_local,
            )
        }
        _ => return,
    };

    // Use the local index first
    let (base_dir, idx_hit) = {
        let s = state.lock();
        let base = s.local_art_dir.clone();

        let k1 = key_title_artist(&track, &artist);

        let hit = s.local_index.get(&k1).cloned().or_else(|| {
            album.as_deref().and_then(|alb| {
                let k2 = key_title_album(&track, alb);
                s.local_index.get(&k2).cloned()
            })
        });

        (base, hit)
    };

    if let Some(audio_path) = idx_hit {
        // Prefer embedded art
        if let Some(out) = extract_embedded_art_to_cache(app, &audio_path) {
            np.artwork_path = Some(out.to_string_lossy().to_string());
            return;
        }
        // Sidecar cover.* in the same folder
        if let Some(dir) = audio_path.parent() {
            if let Some(sidecar) = try_common_names(dir) {
                np.artwork_path = Some(sidecar.to_string_lossy().to_string());
                return;
            }
        }
    }

    // Fallback: your previous best-effort scan using base_dir (if set)
    if let Some(base) = base_dir {
        if let Some(found) = find_local_art_in_base(&base, &artist, album.as_deref(), &track) {
            np.artwork_path = Some(found.to_string_lossy().to_string());
        }
    }
}

#[tauri::command]
async fn get_current_playing(
    state: State<'_, SharedStore>,
    window: tauri::Window,
) -> Result<NowPlaying, String> {
    let client = {
        let guard = state.lock();
        guard
            .client
            .clone()
            .ok_or_else(|| "Not connected to Spotify".to_string())?
    };

    match client
        .current_user_playing_item()
        .await
        .map_err(|e| e.to_string())?
    {
        Some(ctx) => {
            let mut np = build_now_playing_from_ctx(&ctx);
            let app = window.app_handle();
            maybe_set_local_artwork(&app, &state, &mut np, &ctx);
            Ok(np)
        }
        None => Ok(NowPlaying {
            is_playing: false,
            track_name: None,
            artists: vec![],
            album: None,
            artwork_url: None,
            artwork_path: None,
        }),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store: SharedStore = Arc::new(Mutex::new(SpotifyStore::default()));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(store)
        .setup(|app| {
            if let Ok(env_path) = app
                .path()
                .resolve(".env", tauri::path::BaseDirectory::Resource)
            {
                let _ = dotenvy::from_path(env_path);
            }

            let store = app.state::<SharedStore>();
            if let Some(dir) = load_local_art_dir_from_handle(&app.app_handle()) {
                {
                    store.lock().local_art_dir = Some(dir.clone());
                }

                // Build the local index on startup so embedded/sidecar art works right away
                let app_handle = app.app_handle().clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let idx = build_local_index(&dir);
                    let s = app_handle.state::<SharedStore>();
                    let mut g = s.lock();
                    g.local_index = idx;
                    g.art_cache.clear();
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_spotify,
            restore_spotify,
            get_current_playing,
            set_local_art_dir,
            get_local_art_dir,
            write_now_playing_assets,
            get_current_playing_gsmtc,
        ])
        .on_window_event(|window, event| {
            use tauri::WindowEvent;

            match event {
                // Window is actually gone now
                WindowEvent::Destroyed => {
                    let app = window.app_handle();

                    // If no more windows, terminate the app + poller
                    if app.webview_windows().is_empty() {
                        let state = app.state::<SharedStore>();
                        let mut s = state.lock();
                        if let Some(t) = s.cancel.take() {
                            t.cancel();
                        }
                        s.client = None;
                        s.watch_started = false;
                        // let _ = clear_token_cache(&window);

                        // Be absolutely sure the process exits (dev on Windows can be sticky)
                        #[cfg(windows)]
                        {
                            std::process::exit(0);
                        }
                        #[cfg(not(windows))]
                        {
                            app.exit(0);
                        }
                    }
                }

                // Optional: if the **main** window is closed, quit immediately regardless of widget
                WindowEvent::CloseRequested { .. } if window.label() == "main" => {
                    let app = window.app_handle();
                    let state = app.state::<SharedStore>();
                    let mut s = state.lock();
                    if let Some(t) = s.cancel.take() {
                        t.cancel();
                    }
                    s.client = None;
                    s.watch_started = false;
                    // let _ = clear_token_cache(&window);

                    #[cfg(windows)]
                    {
                        std::process::exit(0);
                    }
                    #[cfg(not(windows))]
                    {
                        app.exit(0);
                    }
                }

                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
