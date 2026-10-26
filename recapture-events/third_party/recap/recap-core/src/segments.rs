//! Splitting a game session into matches using `match_start` / `match_end` markers.
//!
//! Games without an API are recorded from launch to exit, menus included. When the
//! user has made "Match start" and/or "Match end" detectors (or an API reports them),
//! the session is cut into one recording per match.

use serde::Serialize;

use crate::events::{kind, GameEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchSegment {
    pub start_ms: u64,
    pub end_ms: u64,
}

pub struct SegmentOptions {
    /// Kept before each match start (e.g. the loading screen).
    pub pad_before_ms: u64,
    /// Kept after each match end (e.g. the victory screen).
    pub pad_after_ms: u64,
    /// Shorter segments are dropped (false positives, aborted queues).
    pub min_len_ms: u64,
    /// Ignore everything before the first match start (the recording resumed in the middle of a
    /// match the player chose to skip).
    pub skip_until_first_start: bool,
}

impl Default for SegmentOptions {
    fn default() -> Self {
        Self {
            pad_before_ms: 5_000,
            pad_after_ms: 8_000,
            min_len_ms: 60_000,
            skip_until_first_start: false,
        }
    }
}

/// Plan match segments. Returns an empty list when the session should stay as it is
/// (no markers, or the only match already spans the whole recording).
pub fn plan_match_segments(events: &[GameEvent], duration_ms: u64, opts: &SegmentOptions) -> Vec<MatchSegment> {
    let mut marks: Vec<&GameEvent> = events
        .iter()
        .filter(|e| e.kind == kind::MATCH_START || e.kind == kind::MATCH_END)
        .collect();
    if marks.is_empty() || duration_ms == 0 {
        return Vec::new();
    }
    marks.sort_by_key(|e| e.t_ms);
    if opts.skip_until_first_start {
        let first = marks.iter().position(|e| e.kind == kind::MATCH_START);
        marks.drain(..first.unwrap_or(marks.len()));
        if marks.is_empty() {
            return Vec::new();
        }
    }

    let mut raw: Vec<(u64, u64)> = Vec::new();
    let mut open: Option<u64> = None;
    let mut last_end: Option<u64> = None;
    for e in marks {
        let t = e.t_ms.min(duration_ms);
        if e.kind == kind::MATCH_START {
            if let Some(s) = open {
                // A new match began before the previous one was seen to end.
                raw.push((s, t));
            }
            open = Some(t);
        } else {
            match open.take() {
                Some(s) => raw.push((s, t)),
                // Only ends are detected: the match ran since the previous end (or the start).
                None => {
                    // Ignore a repeated end marker right after another (e.g. a long results screen).
                    if last_end.is_none_or(|l| t.saturating_sub(l) >= opts.min_len_ms) {
                        raw.push((last_end.unwrap_or(0), t));
                    }
                }
            }
            last_end = Some(t);
        }
    }
    if let Some(s) = open {
        raw.push((s, duration_ms));
    }

    let mut out: Vec<MatchSegment> = raw
        .into_iter()
        .filter(|(s, e)| e.saturating_sub(*s) >= opts.min_len_ms)
        .map(|(s, e)| MatchSegment {
            start_ms: s.saturating_sub(opts.pad_before_ms),
            end_ms: (e + opts.pad_after_ms).min(duration_ms),
        })
        .collect();
    // Padding must not make neighbours overlap.
    for i in 1..out.len() {
        if out[i].start_ms < out[i - 1].end_ms {
            let mid = (out[i].start_ms + out[i - 1].end_ms) / 2;
            out[i - 1].end_ms = mid;
            out[i].start_ms = mid;
        }
    }
    // One match covering (almost) the whole session: nothing to split.
    if out.len() == 1 && out[0].start_ms <= 15_000 && out[0].end_ms + 15_000 >= duration_ms {
        return Vec::new();
    }
    out
}

/// Events inside a segment, rebased so the segment starts at 0.
pub fn events_in_segment(events: &[GameEvent], seg: &MatchSegment) -> Vec<GameEvent> {
    events
        .iter()
        .filter(|e| e.t_ms >= seg.start_ms && e.t_ms <= seg.end_ms)
        .map(|e| {
            let mut e = e.clone();
            e.id = None;
            e.t_ms -= seg.start_ms;
            e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventSource;

    fn ev(k: &str, s: u64) -> GameEvent {
        GameEvent::new(k, s * 1000, EventSource::Vision { detector: "d".into() })
    }

    const MIN: u64 = 60_000;

    #[test]
    fn splits_start_end_pairs_and_drops_menus() {
        let events = vec![
            ev("match_start", 120),
            ev("kill", 300),
            ev("match_end", 1500),
            ev("match_start", 1800),
            ev("match_end", 3000),
        ];
        let segs = plan_match_segments(&events, 3300 * 1000, &SegmentOptions::default());
        assert_eq!(
            segs,
            vec![
                MatchSegment {
                    start_ms: 115_000,
                    end_ms: 1_508_000
                },
                MatchSegment {
                    start_ms: 1_795_000,
                    end_ms: 3_008_000
                },
            ]
        );
        let inside = events_in_segment(&events, &segs[0]);
        assert_eq!(
            inside.iter().map(|e| (e.kind.as_str(), e.t_ms)).collect::<Vec<_>>(),
            vec![("match_start", 5_000), ("kill", 185_000), ("match_end", 1_385_000)]
        );
    }

    #[test]
    fn ends_only_and_starts_only() {
        // Only "VICTORY/DEFEAT" detected: each match runs from the previous end.
        let ends = vec![ev("match_end", 900), ev("match_end", 905), ev("match_end", 2000)];
        let segs = plan_match_segments(
            &ends,
            2100 * 1000,
            &SegmentOptions {
                pad_before_ms: 0,
                pad_after_ms: 0,
                min_len_ms: MIN,
                skip_until_first_start: false,
            },
        );
        assert_eq!(
            segs,
            vec![
                MatchSegment {
                    start_ms: 0,
                    end_ms: 900_000
                },
                MatchSegment {
                    start_ms: 905_000,
                    end_ms: 2_000_000
                }
            ]
        );
        // Only starts: each match runs until the next start / the end of the session.
        let starts = vec![ev("match_start", 100), ev("match_start", 1000)];
        let segs = plan_match_segments(
            &starts,
            1600 * 1000,
            &SegmentOptions {
                pad_before_ms: 0,
                pad_after_ms: 0,
                min_len_ms: MIN,
                skip_until_first_start: false,
            },
        );
        assert_eq!(
            segs,
            vec![
                MatchSegment {
                    start_ms: 100_000,
                    end_ms: 1_000_000
                },
                MatchSegment {
                    start_ms: 1_000_000,
                    end_ms: 1_600_000
                }
            ]
        );
    }

    #[test]
    fn nothing_to_split() {
        assert!(plan_match_segments(&[ev("kill", 10)], 100_000, &SegmentOptions::default()).is_empty());
        // One match that is the whole recording (e.g. an already-split file).
        let whole = vec![ev("match_start", 5), ev("match_end", 1190)];
        assert!(plan_match_segments(&whole, 1200 * 1000, &SegmentOptions::default()).is_empty());
        // A false positive shorter than a minute is ignored.
        let blip = vec![ev("match_start", 100), ev("match_end", 120)];
        assert!(plan_match_segments(&blip, 1200 * 1000, &SegmentOptions::default()).is_empty());
    }

    #[test]
    fn resumed_session_skips_the_rest_of_the_skipped_match() {
        let ev = |k: &str, t: u64| GameEvent::new(k, t, crate::events::EventSource::Manual);
        let events = vec![
            ev(kind::MATCH_END, 90_000),
            ev(kind::MATCH_START, 200_000),
            ev(kind::MATCH_END, 500_000),
        ];
        let opts = SegmentOptions {
            skip_until_first_start: true,
            ..Default::default()
        };
        let segs = plan_match_segments(&events, 600_000, &opts);
        assert_eq!(segs.len(), 1);
        assert_eq!((segs[0].start_ms, segs[0].end_ms), (195_000, 508_000));
        // Only the tail of the skipped match: nothing to keep.
        assert!(plan_match_segments(&events[..1], 600_000, &opts).is_empty());
    }
}
