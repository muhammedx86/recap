//! Turns events into pipe messages: markers right away, clip windows once they close.
//!
//! Overlapping (or touching) clip windows merge into one, so a quick triple kill is one clip.
//! A window is sent when its end has passed, i.e. when the replay buffer already holds all of it.
//! Kills chained as a multikill keep the window open between them, however far apart their own windows are: a
//! window isn't sent while another kill could still join the chain (10 s, 30 s after a quadra kill as in League), so a
//! penta kill spread over 40 s is one clip from before its first kill to after its last.
//! Markers get a window too, sent with `auto: false` unless a clip event is in it: the engine saves a window by its
//! `[games] auto_clips` setting (every `auto` one, or those holding one of the game's big moments).

use recap_core::clips::{window_title, ClipWindow};
use recap_core::events::GameEvent;

use crate::config::Resolved;
use crate::protocol::Message;

/// The next kill of a chain comes this soon after the last one (League's multikill rule).
const CHAIN_MS: u64 = 10_000;
/// After a quadra kill the penta has longer.
const PENTA_MS: u64 = 30_000;

#[derive(Debug, Default)]
pub struct Planner {
    /// The open clip window, in unix ms, with the events inside it and whether one of them asked for a clip.
    open: Option<(ClipWindow, Vec<GameEvent>, bool)>,
    /// The open window's last kill (unix ms) and its multikill count so far.
    chain: Option<(u64, u64)>,
}

fn is_kill(ev: &GameEvent) -> bool {
    ev.kind == "kill" || ev.kind == "multikill"
}

/// Until when after `last` the chain can still go on (a penta kill ends it).
fn chain_until((last, streak): (u64, u64)) -> u64 {
    last + match streak {
        4 => PENTA_MS,
        s if s >= 5 => 0,
        _ => CHAIN_MS,
    }
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
        let chained = is_kill(&ev) && self.chain.is_some_and(|c| ev.t_ms <= chain_until(c));
        if let Some((w, evs, wanted)) = &mut self.open {
            if start <= w.end_ms || chained {
                w.start_ms = w.start_ms.min(start);
                w.end_ms = w.end_ms.max(end);
                if !w.event_kinds.contains(&ev.kind) {
                    w.event_kinds.push(ev.kind.clone());
                }
                self.chain = chain_after(self.chain, &ev);
                evs.push(ev);
                *wanted |= auto;
                return;
            }
        }
        self.flush(out);
        self.chain = chain_after(None, &ev);
        let w = ClipWindow { start_ms: start, end_ms: end, anchor_ms: ev.t_ms, event_kinds: vec![ev.kind.clone()] };
        self.open = Some((w, vec![ev], auto));
    }

    /// Sends the open window if it ended at or before `now_ms`.
    pub fn tick(&mut self, now_ms: u64, out: &mut Vec<Message>) {
        let chain_over = self.chain.is_none_or(|c| chain_until(c) < now_ms);
        if chain_over && self.open.as_ref().is_some_and(|(w, _, _)| w.end_ms <= now_ms) {
            self.flush(out);
        }
    }

    /// Sends the open window now (also on exit: what is in the buffer is still worth saving).
    pub fn flush(&mut self, out: &mut Vec<Message>) {
        self.chain = None;
        if let Some((w, evs, auto)) = self.open.take() {
            let title = window_title(&evs, &w, &label);
            out.push(Message::Clip { start_unix_ms: w.start_ms, end_unix_ms: w.end_ms, title, kinds: w.event_kinds, auto });
        }
    }
}

/// The chain after `ev`: a kill moves its end; a multikill says how many kills it holds.
fn chain_after(chain: Option<(u64, u64)>, ev: &GameEvent) -> Option<(u64, u64)> {
    if !is_kill(ev) {
        return chain;
    }
    let streak = ev.meta.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
    let (last, was) = chain.unwrap_or((ev.t_ms, 0));
    Some((last.max(ev.t_ms), was.max(streak)))
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
        p.tick(114_000, &mut out);
        assert!(clips(&out).is_empty(), "another kill could still join the chain");
        p.tick(114_001, &mut out);
        assert_eq!(clips(&out), vec![(94_000, 107_000, true)]);
        assert_eq!(out.len(), 3, "two events + one clip");
    }

    fn multikill(t: u64, count: u64) -> GameEvent {
        GameEvent::new("multikill", t, EventSource::Api { name: "league-live-client".into() }).with_meta(serde_json::json!({ "count": count }))
    }

    #[test]
    fn a_multikill_spread_over_40_s_is_one_clip_from_before_its_first_kill() {
        let mut p = Planner::default();
        let mut out = Vec::new();
        // kills 10 s apart (their own windows don't touch), the penta 20 s after the quadra
        for (i, t) in [100_000, 110_000, 120_000, 130_000].into_iter().enumerate() {
            p.push(kill(t), CLIP, &mut out);
            if i > 0 {
                p.push(multikill(t, i as u64 + 1), CLIP, &mut out);
            }
            p.tick(t + 9_000, &mut out);
        }
        p.tick(155_000, &mut out);
        assert!(clips(&out).is_empty(), "a penta can still come");
        p.push(kill(150_000), CLIP, &mut out);
        p.push(multikill(150_000, 5), CLIP, &mut out);
        p.tick(152_999, &mut out);
        assert!(clips(&out).is_empty());
        p.tick(153_000, &mut out); // a penta ends the chain: saved 3 s after it
        assert_eq!(clips(&out), vec![(94_000, 153_000, true)]);
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
