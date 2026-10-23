//! Recap core: everything that does not touch Windows APIs or OBS directly.
//!
//! The whole app speaks one language: [`events::GameEvent`]. Vision detectors,
//! official local game APIs, post-match file processors and the manual bookmark
//! hotkey all produce the same event type, so the timeline, auto-clips and
//! library never care where an event came from.

pub mod clips;
pub mod cs2;
pub mod events;
pub mod ffmpeg;
pub mod games;
pub mod league;
pub mod library;
pub mod postmatch;
pub mod protocol;
pub mod scan;
pub mod segments;
pub mod settings;
pub mod vision;
