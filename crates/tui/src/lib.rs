//! t1dm-tui — the ratatui front-end. Rendering is demand-driven: a frame is
//! drawn on input, on a data event, or while an animation is live; otherwise
//! the loop idles blocked on input. `run` owns the terminal for its lifetime.

pub mod anim;
pub mod app;
pub mod boot;
pub mod footer;
pub mod header;
pub mod input;
pub mod layout;
pub mod panes;
pub mod scheduler;
pub mod theme;
pub mod widgets;

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use tokio::sync::broadcast;

use store::Store;
use t1dm_core::{Alert, Config, Photo, PredictionEvent, SampleRow};

use crate::app::App;
use crate::boot::BootSequence;
use crate::scheduler::RenderScheduler;

/// Upper bound on the delta time fed to an animation tick. After a long idle
/// (the loop was blocked on input for minutes) the raw delta would be huge and
/// snap any animation to its end; clamping keeps motion frame-rate independent
/// *and* well-behaved on the first frame after waking.
const MAX_FRAME_DT: Duration = Duration::from_millis(100);

/// A message multiplexed into the render loop from one of the two blocking
/// sources: terminal input and server-pushed data events.
enum LoopMsg {
    Term(Event),
    Data(AppEvent),
}

/// Fallible result for the TUI surface.
pub type Result<T = ()> =
    std::result::Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;

/// How many clients are attached to the WS stream right now. The TUI does not
/// depend on the api crate, so the root binary supplies a closure over the
/// hub's subscriber count — already discounted by the one permanent in-process
/// subscriber the event bridge holds.
pub type ClientProbe = Arc<dyn Fn() -> usize + Send + Sync>;

/// A data event pushed from the server side into the TUI. The root binary
/// maps `api::Event` onto this so the TUI need not depend on `api`.
#[derive(Debug, Clone)]
pub enum AppEvent {
    Sample(SampleRow),
    Prediction(PredictionEvent),
    Photo(Photo),
    Alert(Alert),
    /// A store write that carries no footer indicator of its own (meal, dose,
    /// basal schedule, stats block). Rendering is demand-driven, so without a
    /// wake-up a non-animating layout would keep serving the pre-write frame.
    StoreChanged,
}

/// Run the TUI to completion. Initializes the terminal (raw mode, alternate
/// screen, mouse capture), drives the event loop, and restores on exit —
/// even on error.
pub fn run(
    store: Store,
    cfg: Config,
    rx: broadcast::Receiver<AppEvent>,
    clients: ClientProbe,
) -> Result<()> {
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);

    let result = run_loop(&mut terminal, store, cfg, rx, clients);

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    store: Store,
    cfg: Config,
    rx: broadcast::Receiver<AppEvent>,
    clients: ClientProbe,
) -> Result<()> {
    let show_boot = cfg.ui.show_boot;
    let fps = cfg.ui.fps;
    let mut app = App::new(store, cfg, clients);

    // Both event sources block, so each is bridged onto a single channel by a
    // dedicated thread. The render loop then waits on one queue and can wake
    // for input, a pushed data event, or an animation deadline alike — and
    // sits at zero cost when none of those is pending.
    let (tx, rx_loop) = mpsc::channel::<LoopMsg>();
    spawn_input_reader(tx.clone());
    spawn_event_bridge(rx, tx);

    let mut boot = if show_boot {
        Some(BootSequence::new(app.theme_ref()))
    } else {
        None
    };
    let mut sched = RenderScheduler::new(fps);

    loop {
        let now = Instant::now();

        // Boot owns the frame until it finishes or a key skips it.
        if let Some(b) = &boot {
            if b.done() {
                boot = None;
                sched.request();
            }
        }

        // Serve a frame only when one is actually due (dirty or animating, and
        // past the fps-derived interval). Delta time is measured by the
        // scheduler from the last *served* frame, so animation stays identical
        // across frame rates.
        if let Some(dt) = sched.take_due(now) {
            let dt = dt.min(MAX_FRAME_DT);
            let animating = if boot.is_some() { true } else { app.tick(dt) };
            sched.set_animating(animating);
            terminal.draw(|frame| {
                if let Some(b) = &boot {
                    b.render(frame, frame.area(), app.theme_ref());
                } else {
                    app.draw(frame);
                }
            })?;
        }

        // Block until the next event, or until the next animation frame comes
        // due. `None` means nothing is pending — block indefinitely (idle).
        let received = match sched.poll_timeout(Instant::now()) {
            Some(timeout) => match rx_loop.recv_timeout(timeout) {
                Ok(msg) => Some(msg),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx_loop.recv() {
                Ok(msg) => Some(msg),
                Err(_) => break,
            },
        };

        if let Some(msg) = received {
            if handle_msg(&mut app, &mut boot, &mut sched, msg) {
                break;
            }
        }
    }

    Ok(())
}

/// Apply one multiplexed message; returns true when the app should quit.
fn handle_msg(
    app: &mut App,
    boot: &mut Option<BootSequence>,
    sched: &mut RenderScheduler,
    msg: LoopMsg,
) -> bool {
    match msg {
        LoopMsg::Term(Event::Key(key)) if key.kind == KeyEventKind::Press => {
            if boot.is_some() {
                // Any key skips the boot sequence and is otherwise swallowed.
                *boot = None;
            } else if app.on_key(key) {
                return true;
            }
            sched.request();
        }
        LoopMsg::Term(Event::Mouse(m)) => {
            if boot.is_none() && app.on_mouse(m) {
                return true;
            }
            sched.request();
        }
        LoopMsg::Term(Event::Resize(_, _)) => sched.request(),
        LoopMsg::Term(_) => {}
        LoopMsg::Data(ev) => {
            app.apply_event(ev);
            sched.request();
        }
    }
    false
}

/// Read terminal events on a dedicated thread and forward them onto the loop
/// channel. Exits when the loop drops the receiver or stdin closes.
fn spawn_input_reader(tx: mpsc::Sender<LoopMsg>) {
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if tx.send(LoopMsg::Term(ev)).is_err() {
                break;
            }
        }
    });
}

/// Block on the broadcast of server-pushed events and forward them onto the
/// loop channel. Runs on a plain thread (no tokio runtime), so `blocking_recv`
/// is sound here.
fn spawn_event_bridge(mut rx: broadcast::Receiver<AppEvent>, tx: mpsc::Sender<LoopMsg>) {
    std::thread::spawn(move || loop {
        match rx.blocking_recv() {
            Ok(ev) => {
                if tx.send(LoopMsg::Data(ev)).is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    });
}
