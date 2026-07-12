//! Demand-driven render scheduler. A frame is drawn on input, on a data
//! event, or while an animation is live; otherwise the loop idles. The fps
//! value is a *ceiling*, never a target.

use std::time::{Duration, Instant};

pub struct RenderScheduler {
    /// Config-capped frame-rate ceiling.
    pub fps_cap: u16,
    last_frame: Instant,
    /// A redraw has been requested (input/event) but not yet served.
    dirty: bool,
    /// An animation is live; keep drawing until it settles.
    animating: bool,
}

impl RenderScheduler {
    pub fn new(fps_cap: u16) -> Self {
        RenderScheduler {
            fps_cap: fps_cap.max(1),
            last_frame: Instant::now(),
            dirty: true,
            animating: false,
        }
    }

    /// Minimum interval between frames implied by the fps ceiling.
    pub fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.fps_cap.max(1) as f64)
    }

    /// Request a one-off redraw (input arrived, data event, resize).
    pub fn request(&mut self) {
        self.dirty = true;
    }

    /// Declare whether an animation is currently live.
    pub fn set_animating(&mut self, animating: bool) {
        self.animating = animating;
        if animating {
            self.dirty = true;
        }
    }

    /// True when the loop should idle-block for input rather than spin.
    pub fn is_idle(&self) -> bool {
        !self.dirty && !self.animating
    }

    /// If a frame is due (dirty/animating and past the fps interval), consume
    /// the dirty flag and return the delta time since the last served frame.
    pub fn take_due(&mut self, now: Instant) -> Option<Duration> {
        if self.is_idle() {
            return None;
        }
        let dt = now.saturating_duration_since(self.last_frame);
        if dt < self.frame_interval() {
            return None;
        }
        self.last_frame = now;
        self.dirty = false;
        Some(dt)
    }

    /// How long the event loop may block waiting for input before it must
    /// wake to advance an animation. `None` means block indefinitely.
    pub fn poll_timeout(&self, now: Instant) -> Option<Duration> {
        if self.is_idle() {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.last_frame);
        Some(self.frame_interval().saturating_sub(elapsed))
    }
}
