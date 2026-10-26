//! Turns events into pipe messages: markers right away, clip windows once they close.
//!
//! Overlapping (or touching) clip windows merge into one, so a quick triple kill is one clip.
//! A window is sent when its end has passed, i.e. when the replay buffer already holds all of it.
//! Markers get a window too, sent with `auto: false` unless a clip event is in it: the engine saves a window by its
//! `[games] auto_clips` setting (every `auto` one, or those holding one of the game's big moments).

use recap_core::clips::{window_title, ClipWindow};
use recap_core::events::GameEvent;

use crate::config::Resolved;
use crate::protocol::Message;

#[derive(Debug, Default)]
pub struct Planner {
    /// The open clip window, in unix ms, with the events inside it and whether one of them asked for a clip.
    open: Option<(ClipWindow, Vec<GameEvent>, bool)>,
}

impl Planner {
    /// `ev.t_ms` is unix ms here.
    pub fn push(&mut self, ev: GameEvent, action: Resolved, out: &mut Vec<Message>) {
        let (before_ms, after_ms, auto) = match action {
            Resolved::Ignore => return,
            Resolved::Marker { before_ms, after_ms } => (before_ms, after_ms, false),
            Resolved::Clip { before_ms, after_ms } => (before_ms, after_ms, true),
        };
        out.push(Message::event(&ev));
        let start = ev.t_ms.saturating_sub(before_ms);
        let end = ev.t_ms + after_ms;
        if let Some((w, evs, wanted)) = &mut self.open {
            if start <= w.end_ms {
                w.start_ms = w.start_ms.min(start);
                w.end_ms = w.end_ms.max(end);
                if !w.event_kinds.contains(&ev.kind) {
                    w.event_kinds.push(ev.kind.clone());
                }
                evs.push(ev);
                *wanted |= auto;
                return;
            }
        }
        self.flush(out);
        let w = ClipWindow { start_ms: start, end_ms: end, anchor_ms: ev.t_ms, event_kinds: vec![ev.kind.clone()] };
        self.open = Some((w, vec![ev], auto));
    }

    /// Sends the open window if it ended at or before `now_ms`.
    pub fn tick(&mut self, now_ms: u64, out: &mut Vec<Message>) {
        if self.open.as_ref().is_some_and(|(w, _, _)| w.end_ms <= now_ms) {
            self.flush(out);
        }
    }

    /// Sends the open window now (also on exit: what is in the buffer is still worth saving).
    pub fn flush(&mut self, out: &mut Vec<Message>) {
        if let Some((w, evs, auto)) = self.open.take() {
            let title = window_title(&evs, &w, &label);
            out.push(Message::Clip { start_unix_ms: w.start_ms, end_unix_ms: w.end_ms, title, kinds: w.event_kinds, auto });
        }
    }
}

fn label(kind: &str, plural: bool) -> String {
    let name = kind.replace('_', " ");
    if plural { format!("{name}s") } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recap_core::events::EventSource;

    fn kill(t: u64) -> GameEvent {
        GameEvent::new("kill", t, EventSource::Api { name: "cs2-gsi".into() })
    }
    const CLIP: Resolved = Resolved::Clip { before_ms: 6000, after_ms: 3000 };
    const MARKER: Resolved = Resolved::Marker { before_ms: 6000, after_ms: 3000 };

    fn clips(out: &[Message]) -> Vec<(u64, u64, bool)> {
        out.iter()
            .filter_map(|m| match m {
                Message::Clip { start_unix_ms, end_unix_ms, auto, .. } => Some((*start_unix_ms, *end_unix_ms, *auto)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn close_kills_merge_into_one_clip_sent_after_it_ends() {
        let mut p = Planner::default();
        let mut out = Vec::new();
        p.push(kill(100_000), CLIP, &mut out);
        p.push(kill(104_000), CLIP, &mut out);
        p.tick(106_999, &mut out);
        assert!(clips(&out).is_empty());
        p.tick(107_000, &mut out);
        assert_eq!(clips(&out), vec![(94_000, 107_000, true)]);
        assert_eq!(out.len(), 3, "two events + one clip");
    }

    #[test]
    fn far_apart_kills_are_two_clips() {
        let mut p = Planner::default();
        let mut out = Vec::new();
        p.push(kill(100_000), CLIP, &mut out);
        p.push(kill(200_000), CLIP, &mut out);
        p.flush(&mut out);
        assert_eq!(clips(&out), vec![(94_000, 103_000, true), (194_000, 203_000, true)]);
    }

    #[test]
    fn a_marker_window_is_sent_not_asked_for_unless_a_clip_joins_it() {
        let mut p = Planner::default();
        let mut out = Vec::new();
        p.push(kill(1000), MARKER, &mut out);
        p.push(kill(100_000), MARKER, &mut out);
        p.push(kill(101_000), CLIP, &mut out);
        p.push(kill(200_000), Resolved::Ignore, &mut out);
        p.flush(&mut out);
        assert_eq!(clips(&out), vec![(0, 4000, false), (94_000, 104_000, true)]);
        assert_eq!(out.len(), 5, "three events + two clips");
    }
}
