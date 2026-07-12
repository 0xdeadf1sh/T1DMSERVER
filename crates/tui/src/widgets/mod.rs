//! Reusable TUI widgets. Each lives in its own file so downstream agents can
//! own them independently.

pub mod biotime;
pub mod chart;
pub mod fan;
pub mod image;
pub mod panel;
pub mod qr;
pub mod scroll;
pub mod strips;

pub use biotime::BioTimeGauge;
pub use chart::BgChart;
pub use fan::FanWidget;
pub use scroll::ScrollState;
pub use strips::StripWidget;
