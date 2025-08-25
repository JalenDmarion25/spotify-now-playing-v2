use parking_lot::Mutex;
use rspotify::{
    clients::{BaseClient, OAuthClient},
    model::{Image, PlayableItem},
    scopes, AuthCodePkceSpotify, Config, Credentials, OAuth, Token,
};
use serde::Serialize;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::Arc,
};
use tauri::{Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Default)]
struct SpotifyStore {
    // Use Arc so we can clone a handle and drop the lock before we await (fixes the Send error)
    client: Option<Arc<AuthCodePkceSpotify>>,
    watch_started: bool,
    cancel: Option<CancellationToken>,
}

type SharedStore = Arc<Mutex<SpotifyStore>>;

#[derive(Serialize)]
struct NowPlaying {
    is_playing: bool,
    track_name: Option<String>,
    artists: Vec<String>,
    album: Option<String>,
    artwork_url: Option<String>,
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
    }
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

                match client.current_user_playing_item().await {
                  Ok(Some(ctx)) => {
                    let np = build_now_playing_from_ctx(&ctx);
                    let _ = app.emit("now_playing_update", &np);
                  }
                  Ok(None) => {
                    let _ = app.emit("now_playing_update", &NowPlaying {
                      is_playing: false, track_name: None, artists: vec![], album: None, artwork_url: None
                    });
                  }
                  Err(_) => {
                    // treat as fatal (likely revoked / invalid)
                    let _ = app.emit("auth_lost", &());
                    let mut s = state_handle.lock();
                    s.client = None;
                    s.watch_started = false;
                    s.cancel = None;
                    return;
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

#[tauri::command]
async fn get_current_playing(state: State<'_, SharedStore>) -> Result<NowPlaying, String> {
    // Clone an Arc so the lock is dropped BEFORE we await (fixes !Send error)
    let client = {
        let guard = state.lock();
        guard
            .client
            .clone()
            .ok_or_else(|| "Not connected to Spotify".to_string())?
    };

    let playing = client
        .current_user_playing_item()
        .await
        .map_err(|e| e.to_string())?;

    if let Some(ctx) = playing {
        let is_playing = ctx.is_playing;

        let mut track_name = None;
        let mut artists = Vec::new();
        let mut album = None;
        let mut artwork_url = None;

        if let Some(item) = ctx.item {
            match item {
                PlayableItem::Track(track) => {
                    track_name = Some(track.name.clone());
                    artists = track.artists.into_iter().map(|a| a.name).collect();
                    album = Some(track.album.name.clone());
                    artwork_url = pick_image_url(&track.album.images, 300);
                }
                PlayableItem::Episode(ep) => {
                    // In case user is listening to a podcast episode
                    track_name = Some(ep.name.clone());
                    album = Some(ep.show.name.clone());
                    artists = vec![ep.show.publisher.clone()];
                    artwork_url = pick_image_url(&ep.images, 300);
                }
            }
        }

        Ok(NowPlaying {
            is_playing,
            track_name,
            artists,
            album,
            artwork_url,
        })
    } else {
        Ok(NowPlaying {
            is_playing: false,
            track_name: None,
            artists: Vec::new(),
            album: None,
            artwork_url: None,
        })
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let store: SharedStore = Arc::new(Mutex::new(SpotifyStore::default()));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(store)
        .setup(|app| {
            // Load .env from the app's bundled resources dir
            if let Ok(env_path) = app
                .path()
                .resolve(".env", tauri::path::BaseDirectory::Resource)
            {
                let _ = dotenvy::from_path(env_path);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_spotify,
            restore_spotify,
            get_current_playing
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
                        let _ = clear_token_cache(&window);

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
                    let _ = clear_token_cache(&window);

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
