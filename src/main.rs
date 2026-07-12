//! T1DMSERVER entrypoint. Wires config → Store → axum router (served by
//! tokio) and the ratatui TUI, bridging the WS hub's events into the TUI and
//! filesystem-watching the models directory. The TUI owns the main thread;
//! the HTTP/WS server and watchers run on a multi-thread tokio runtime.

use std::path::PathBuf;

use anyhow::Context;
use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;

use api::{Event, WsHub};
use store::Store;
use t1dm_core::Config;
use tui::AppEvent;

fn main() -> anyhow::Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./config.toml"));

    let cfg = load_config(&config_path);

    // The TUI owns the terminal, so tracing must not write to stderr (it would
    // corrupt the frame). Forward records into the in-memory ring the Logs pane
    // reads; ANSI is disabled so the ring holds clean text.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.log.level)),
        )
        .with_ansi(false)
        .with_writer(|| tui::panes::logs::LogSink)
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    // Open the store and do a startup model scan.
    let store = Store::open(&cfg).context("opening store")?;
    let models_dir = store.models_dir();
    if let Err(e) = store.refresh_models(&models_dir) {
        tracing::warn!("initial model scan failed: {e}");
    }

    // The WS hub and the TUI update channel; a bridge maps hub events onto
    // TUI events so the TUI surface never depends on the api crate.
    let hub = WsHub::new(1024);
    let (tui_tx, tui_rx) = broadcast::channel::<AppEvent>(1024);
    rt.spawn(bridge_events(hub.clone(), tui_tx));

    // HTTP/WS server.
    let router = api::build_router(store.clone(), hub.clone());
    let addr = format!("{}:{}", cfg.server.bind, cfg.server.port);
    rt.spawn(async move {
        tracing::info!("serving on {addr}");
        if let Err(e) = api::serve(router, &addr).await {
            tracing::error!("server exited: {e}");
        }
    });

    // Daily statistics cache: compute all windows once at startup, then on a
    // fixed TTL cadence, so `GET /v1/stats` serves a pre-warmed result.
    let stats_store = store.clone();
    rt.spawn(async move {
        let period = std::time::Duration::from_millis(store::STATS_CACHE_TTL_MS as u64);
        let mut tick = tokio::time::interval(period);
        loop {
            tick.tick().await;
            let s = stats_store.clone();
            match tokio::task::spawn_blocking(move || s.refresh_stats_cache()).await {
                Ok(Ok(())) => tracing::debug!("stats cache refreshed"),
                Ok(Err(e)) => tracing::warn!("stats cache refresh failed: {e}"),
                Err(e) => tracing::warn!("stats cache refresh task panicked: {e}"),
            }
        }
    });

    // Filesystem watch on the models directory (kept alive until shutdown).
    let _watcher = match start_model_watcher(store.clone()) {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::warn!("model watcher disabled: {e}");
            None
        }
    };

    // The TUI runs on the main thread and owns the terminal for its lifetime.
    let tui_result = tui::run(store, cfg, tui_rx);

    rt.shutdown_background();
    tui_result.map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(())
}

/// Load config from `path`, falling back to defaults if it is absent/invalid.
fn load_config(path: &std::path::Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!(
                    "warning: failed to parse {}: {e}; using defaults",
                    path.display()
                );
                Config::default()
            }
        },
        Err(_) => {
            eprintln!("note: {} not found; using defaults", path.display());
            Config::default()
        }
    }
}

/// Subscribe to the WS hub and forward each event to the TUI channel.
async fn bridge_events(hub: WsHub, tui_tx: broadcast::Sender<AppEvent>) {
    let mut rx = hub.subscribe();
    loop {
        match rx.recv().await {
            Ok(msg) => {
                let ev = match msg.event {
                    Event::Sample(s) => AppEvent::Sample(s),
                    Event::Prediction(p) => AppEvent::Prediction(p),
                    Event::Note(n) => AppEvent::Note(n),
                    Event::Photo(p) => AppEvent::Photo(p),
                    Event::Alert(a) => AppEvent::Alert(a),
                };
                let _ = tui_tx.send(ev);
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Watch the models directory; rescan the store on any change.
fn start_model_watcher(store: Store) -> anyhow::Result<notify::RecommendedWatcher> {
    use notify::{recommended_watcher, Event, RecursiveMode, Watcher};

    let dir = store.models_dir();
    std::fs::create_dir_all(&dir).ok();
    let s = store.clone();
    let mut watcher = recommended_watcher(move |res: notify::Result<Event>| {
        if res.is_ok() {
            if let Err(e) = s.refresh_models(&s.models_dir()) {
                tracing::warn!("model rescan failed: {e}");
            }
        }
    })
    .context("creating fs watcher")?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", dir.display()))?;
    Ok(watcher)
}
