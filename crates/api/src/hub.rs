//! The WebSocket broadcast hub. The server pushes typed [`Event`]s to every
//! connected session; alerts are fanned out to all sessions *except* those of
//! the originating token.

use serde::Serialize;
use tokio::sync::broadcast;

use t1dm_core::{CgmSourceRow, 
    Alert, BasalSchedule, DoseEvent, MealEvent, Photo, PredictionEvent, SampleRow, StatsBlock,
};

/// Server→client push events, serialized as tagged JSON with a `"type"` key.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Event {
    Sample(SampleRow),
    Prediction(PredictionEvent),
    Photo(Photo),
    Alert(Alert),
    Meal(MealEvent),
    Dose(DoseEvent),
    #[serde(rename = "basal_schedule")]
    BasalSchedule(BasalSchedule),
    /// A struct variant, not `CgmSource(Vec<CgmSourceRow>)`. [`Event`] is internally tagged, and
    /// serde cannot inject the `"type"` key into a value that serialises as a sequence — a newtype
    /// variant wrapping a `Vec` fails at RUNTIME with "cannot serialize tagged newtype variant",
    /// which the WS send loop discards silently. Naming the field gives the tag a map to live in.
    #[serde(rename = "cgm_source")]
    CgmSource { sources: Vec<CgmSourceRow> },
    Stats(StatsBlock),
}

/// An enveloped event carrying an optional token id to exclude from delivery
/// (used so an alert never echoes back to its originator's sessions).
#[derive(Debug, Clone)]
pub struct HubMsg {
    pub event: Event,
    pub exclude_token: Option<i64>,
}

impl HubMsg {
    /// Whether this message should be delivered to the session belonging to
    /// `token_id`. An alert never echoes back to its originator's sessions.
    pub fn delivers_to(&self, token_id: i64) -> bool {
        self.exclude_token != Some(token_id)
    }
}

/// Broadcast fan-out hub. Cloneable; every clone shares one channel.
#[derive(Clone)]
pub struct WsHub {
    tx: broadcast::Sender<HubMsg>,
}

impl WsHub {
    /// Create a hub with the given channel capacity (per-subscriber buffer).
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        WsHub { tx }
    }

    /// Subscribe a new WS connection to the stream.
    pub fn subscribe(&self) -> broadcast::Receiver<HubMsg> {
        self.tx.subscribe()
    }

    /// Number of currently connected subscribers.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Broadcast an event to every connected session.
    pub fn broadcast(&self, event: Event) {
        let _ = self.tx.send(HubMsg {
            event,
            exclude_token: None,
        });
    }

    /// Broadcast an event to every session except those of `token_id`.
    pub fn broadcast_except(&self, event: Event, token_id: i64) {
        let _ = self.tx.send(HubMsg {
            event,
            exclude_token: Some(token_id),
        });
    }
}

impl Default for WsHub {
    fn default() -> Self {
        WsHub::new(1024)
    }
}
