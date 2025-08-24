use parking_lot::Mutex;
use rspotify::{
    clients::OAuthClient,
    model::{Image, PlayableItem},
    scopes, AuthCodePkceSpotify, Config, Credentials, OAuth,
};
use serde::Serialize;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
};
use tauri::{Manager, State};
use url::Url;

#[derive(Default)]
struct SpotifyStore {
    // Use Arc so we can clone a handle and drop the lock before we await (fixes the Send error)
    client: Option<Arc<AuthCodePkceSpotify>>,
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

#[tauri::command]
async fn connect_spotify(state: State<'_, SharedStore>) -> Result<(), String> {
    // 1) Credentials + OAuth (PKCE: no secret required; empty string is fine)
    let client_id =
        std::env::var("SPOTIFY_CLIENT_ID").map_err(|_| "Missing SPOTIFY_CLIENT_ID".to_string())?;
    let redirect_uri = "http://127.0.0.1:5173/callback".to_string();

    // rspotify 0.15 -> new(&str, &str)
    let creds = Credentials::new(&client_id, "");

    let oauth = OAuth {
        redirect_uri: redirect_uri.clone(),
        scopes: scopes!("user-read-currently-playing", "user-read-playback-state"),
        ..Default::default()
    };
    let config = Config {
        token_cached: false,
        ..Default::default()
    };

    let mut spotify = AuthCodePkceSpotify::with_config(creds, oauth, config);

    // 2) Open authorize URL
    let auth_url = spotify.get_authorize_url(None).map_err(|e| e.to_string())?;
    tauri_plugin_opener::open_url(auth_url.as_str(), None::<&str>).map_err(|e| e.to_string())?;

    // 3) Tiny blocking HTTP listener to catch ?code=...
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let addr = "127.0.0.1:5173".to_string();

    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = run_callback_server_blocking(&addr, tx) {
            eprintln!("Callback server error: {e}");
        }
    });

    // 4) Wait for code -> exchange for token
    let code = rx.await.map_err(|e| format!("Callback wait error: {e}"))?;

    spotify
        .request_token(&code)
        .await
        .map_err(|e| format!("Token exchange failed: {e}"))?;

    // 5) Save client (Arc) in state
    {
        let mut guard = state.lock();
        guard.client = Some(Arc::new(spotify));
    }

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
            artwork_url, // <--- NEW
        })
    } else {
        Ok(NowPlaying {
            is_playing: false,
            track_name: None,
            artists: Vec::new(),
            album: None,
            artwork_url: None, // <--- NEW
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
            get_current_playing
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
