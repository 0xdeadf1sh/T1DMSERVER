//! Delta-time animation primitives. Every animation is dt-scaled so it looks
//! identical at 30 or 120 fps. Themes supply the *style* (easing, glyphs);
//! these helpers supply the *mechanics*: easing curves, wrapping oscillators,
//! colour blending, and a couple of small stateful timelines. Nothing here is
//! theme-specific — the theme layer composes these into its signature look.

use std::f32::consts::PI;
use std::time::Duration;

use ratatui::style::Color;

// ---------------------------------------------------------------------------
// Scalar interpolation & oscillators
// ---------------------------------------------------------------------------

/// Linear interpolation, `t` clamped to 0..=1.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// A triangle pulse in 0..=1 from a wrapping phase in 0..=1 (peaks at 0.5).
#[inline]
pub fn pulse(phase: f32) -> f32 {
    let p = phase.rem_euclid(1.0);
    if p < 0.5 {
        p * 2.0
    } else {
        (1.0 - p) * 2.0
    }
}

/// A rising sawtooth in 0..=1 from a wrapping phase (for scrolling marquees).
#[inline]
pub fn sawtooth(phase: f32) -> f32 {
    phase.rem_euclid(1.0)
}

/// A sine remapped to 0..=1 over one wrapping phase (smooth breathing).
#[inline]
pub fn sine01(phase: f32) -> f32 {
    0.5 + 0.5 * (2.0 * PI * phase).sin()
}

/// A square wave in {0.0, 1.0}; on for the first `duty` fraction of the phase.
/// The mechanical, hard-edged sibling of [`sine01`].
#[inline]
pub fn blink(phase: f32, duty: f32) -> f32 {
    if phase.rem_euclid(1.0) < duty.clamp(0.0, 1.0) {
        1.0
    } else {
        0.0
    }
}

/// A decaying multi-flash envelope in 0..=1 across a single 0..=1 progress:
/// `flashes` bright pulses that taper to nothing. Suited to alert bursts.
#[inline]
pub fn flash(progress: f32, flashes: f32) -> f32 {
    let t = progress.clamp(0.0, 1.0);
    (PI * flashes * t).sin().abs() * (1.0 - t)
}

/// Phase-shifted breathing intensity for fan tier `tier`, so stacked tiers
/// shimmer out of step rather than in unison.
#[inline]
pub fn shimmer(tier: usize, phase: f32) -> f32 {
    sine01(phase + tier as f32 * 0.17)
}

/// A precise wall-clock phase in 0..=1 for a loop of `period_ms`, computed in
/// integer space first so it stays exact at epoch-millisecond magnitudes
/// (an `i64 as f32` would quantise to useless buckets).
#[inline]
pub fn phase(now_ms: i64, period_ms: i64) -> f32 {
    if period_ms <= 0 {
        return 0.0;
    }
    now_ms.rem_euclid(period_ms) as f32 / period_ms as f32
}

/// Map a value within `[lo, hi]` to a spark-glyph index in 0..=7.
#[inline]
pub fn spark_index(v: f32, lo: f32, hi: f32) -> usize {
    if hi <= lo {
        return 0;
    }
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    (t * 7.0).round() as usize
}

// ---------------------------------------------------------------------------
// Easing library — inputs and outputs nominally in 0..=1
// ---------------------------------------------------------------------------

/// Classic Hermite smoothstep; the neutral default easing.
#[inline]
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Quadratic ease-in — slow start, crisp finish (a derezz snap).
#[inline]
pub fn ease_in_quad(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t
}

/// Cubic ease-out — fast start, gentle settle.
#[inline]
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Back ease-out — overshoots past 1.0 then settles (a playful bounce). May
/// briefly exceed 1.0 by design; clamp at the call site if a bound is needed.
#[inline]
pub fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c1 = 1.701_58_f32;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// Bounce ease-out — decaying elastic hops toward 1.0.
#[inline]
pub fn ease_out_bounce(t: f32) -> f32 {
    let mut t = t.clamp(0.0, 1.0);
    let n1 = 7.5625_f32;
    let d1 = 2.75_f32;
    if t < 1.0 / d1 {
        n1 * t * t
    } else if t < 2.0 / d1 {
        t -= 1.5 / d1;
        n1 * t * t + 0.75
    } else if t < 2.5 / d1 {
        t -= 2.25 / d1;
        n1 * t * t + 0.9375
    } else {
        t -= 2.625 / d1;
        n1 * t * t + 0.984_375
    }
}

// ---------------------------------------------------------------------------
// Colour mechanics
// ---------------------------------------------------------------------------

#[inline]
fn to_rgb(c: Color) -> Option<(f32, f32, f32)> {
    match c {
        Color::Rgb(r, g, b) => Some((r as f32, g as f32, b as f32)),
        _ => None,
    }
}

/// Linearly blend two colours, `t` in 0..=1 (0 → `a`, 1 → `b`). Non-RGB colours
/// have no numeric channels, so they snap at the midpoint rather than blend.
#[inline]
pub fn blend(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (to_rgb(a), to_rgb(b)) {
        (Some((ar, ag, ab)), Some((br, bg, bb))) => Color::Rgb(
            (ar + (br - ar) * t) as u8,
            (ag + (bg - ag) * t) as u8,
            (ab + (bb - ab) * t) as u8,
        ),
        _ => {
            if t < 0.5 {
                a
            } else {
                b
            }
        }
    }
}

/// Scale a colour's brightness by `k` (channels clamped to 0..=255). `k > 1`
/// blooms toward white-hot; `k < 1` dims — the raw material of a glow pulse.
#[inline]
pub fn scale(c: Color, k: f32) -> Color {
    let k = k.max(0.0);
    match to_rgb(c) {
        Some((r, g, b)) => Color::Rgb(
            (r * k).clamp(0.0, 255.0) as u8,
            (g * k).clamp(0.0, 255.0) as u8,
            (b * k).clamp(0.0, 255.0) as u8,
        ),
        None => c,
    }
}

// ---------------------------------------------------------------------------
// Stateful, dt-driven helpers
// ---------------------------------------------------------------------------

/// A one-shot timeline that advances by delta time and reports progress.
#[derive(Debug, Clone, Copy)]
pub struct Timeline {
    elapsed: Duration,
    duration: Duration,
}

impl Timeline {
    pub fn new(duration: Duration) -> Self {
        Timeline {
            elapsed: Duration::ZERO,
            duration,
        }
    }

    /// Advance by `dt`; returns true once (and while) finished.
    pub fn advance(&mut self, dt: Duration) -> bool {
        self.elapsed = (self.elapsed + dt).min(self.duration);
        self.finished()
    }

    /// Progress in 0..=1.
    pub fn progress(&self) -> f32 {
        if self.duration.is_zero() {
            1.0
        } else {
            (self.elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
        }
    }

    pub fn finished(&self) -> bool {
        self.elapsed >= self.duration
    }

    pub fn reset(&mut self) {
        self.elapsed = Duration::ZERO;
    }
}

/// A free-running phase accumulator for looping animations (live-point pulse,
/// fan shimmer, sparkline scroll). Advance it by `dt`; read the shaped value.
#[derive(Debug, Clone, Copy)]
pub struct Pulse {
    phase: f32,
    period: f32,
}

impl Pulse {
    /// A pulse completing one wrap every `period_secs`.
    pub fn new(period_secs: f32) -> Self {
        Pulse {
            phase: 0.0,
            period: period_secs.max(1e-3),
        }
    }

    /// Advance the wrapping phase by `dt` (delta-time scaled).
    pub fn advance(&mut self, dt: Duration) {
        self.phase = (self.phase + dt.as_secs_f32() / self.period).rem_euclid(1.0);
    }

    /// The raw phase in 0..=1.
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// A triangle reading (linear up/down).
    pub fn triangle(&self) -> f32 {
        pulse(self.phase)
    }

    /// A smooth sinusoidal reading.
    pub fn sine(&self) -> f32 {
        sine01(self.phase)
    }
}

/// A directional pane transition built on a [`Timeline`]. `forward` distinguishes
/// a Tab (advance) from a BackTab (retreat) so the swap can slide the right way.
#[derive(Debug, Clone, Copy)]
pub struct Transition {
    tl: Timeline,
    forward: bool,
}

impl Transition {
    pub fn new(duration: Duration, forward: bool) -> Self {
        Transition {
            tl: Timeline::new(duration),
            forward,
        }
    }

    /// Advance by `dt`; returns true once finished.
    pub fn advance(&mut self, dt: Duration) -> bool {
        self.tl.advance(dt)
    }

    pub fn progress(&self) -> f32 {
        self.tl.progress()
    }

    pub fn finished(&self) -> bool {
        self.tl.finished()
    }

    /// +1.0 for a forward transition, -1.0 for a backward one.
    pub fn direction(&self) -> f32 {
        if self.forward {
            1.0
        } else {
            -1.0
        }
    }
}
