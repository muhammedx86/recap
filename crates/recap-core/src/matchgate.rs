//! Record each match separately while a game stays open.
//!
//! Some games say when you are in a match (VALORANT through the Riot Client's local presence).
//! [`MatchGate`] turns that signal into start/stop decisions:
//!
//! * a match begins -> start recording (unless the player stopped this match by hand);
//! * the match ends -> stop after a short tail, so end screens make it into the recording;
//! * a manual stop skips only the current match; the next one records again.

/// What the caller should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    None,
    Start,
    Stop,
}

#[derive(Debug, Clone)]
pub struct MatchGate {
    /// Keep recording this long after the match ends.
    pub tail_ms: u64,
    in_match: bool,
    /// Counts matches (increments when one begins).
    seq: u64,
    out_since_ms: Option<u64>,
    skip_seq: Option<u64>,
}

impl MatchGate {
    pub fn new(tail_ms: u64) -> Self {
        Self {
            tail_ms,
            in_match: false,
            seq: 0,
            out_since_ms: None,
            skip_seq: None,
        }
    }

    /// Feed the latest in-match signal.
    pub fn observe(&mut self, in_match: bool, now_ms: u64) {
        if in_match && !self.in_match {
            self.seq += 1;
        }
        if !in_match && self.in_match {
            self.out_since_ms = Some(now_ms);
        }
        if in_match {
            self.out_since_ms = None;
        }
        self.in_match = in_match;
    }

    pub fn in_match(&self) -> bool {
        self.in_match
    }

    /// Decide given whether this game is being recorded right now.
    pub fn decide(&self, recording: bool, now_ms: u64) -> GateAction {
        if !recording && self.in_match && self.skip_seq != Some(self.seq) {
            GateAction::Start
        } else if recording && !self.in_match && self.out_since_ms.is_some_and(|t| now_ms.saturating_sub(t) >= self.tail_ms) {
            GateAction::Stop
        } else {
            GateAction::None
        }
    }

    /// The player stopped the recording: do not restart until the next match.
    pub fn skip_current(&mut self) {
        if self.in_match {
            self.skip_seq = Some(self.seq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use GateAction::*;

    #[test]
    fn records_each_match_and_respects_manual_stops() {
        let mut g = MatchGate::new(3000);
        // Menus: nothing to do.
        g.observe(false, 0);
        assert_eq!(g.decide(false, 0), None);
        // Match 1 begins.
        g.observe(true, 1000);
        assert_eq!(g.decide(false, 1000), Start);
        assert_eq!(g.decide(true, 2000), None);
        // Match ends: stop only after the tail.
        g.observe(false, 10_000);
        assert_eq!(g.decide(true, 11_000), None);
        assert_eq!(g.decide(true, 13_000), Stop);
        assert_eq!(g.decide(false, 14_000), None);
        // Match 2: records again without relaunching the game.
        g.observe(true, 20_000);
        assert_eq!(g.decide(false, 20_000), Start);
        // The player stops it by hand: stays stopped for this match...
        g.skip_current();
        assert_eq!(g.decide(false, 25_000), None);
        g.observe(true, 26_000);
        assert_eq!(g.decide(false, 26_000), None);
        // ...and the next match records again.
        g.observe(false, 30_000);
        g.observe(true, 40_000);
        assert_eq!(g.decide(false, 40_000), Start);
    }

    #[test]
    fn brief_flicker_out_of_a_match_does_not_stop() {
        let mut g = MatchGate::new(3000);
        g.observe(true, 0);
        g.observe(false, 5000);
        g.observe(true, 6000);
        assert_eq!(g.decide(true, 9000), None);
    }
}

/// Marvel Rivals writes `%LOCALAPPDATA%\Marvel\Saved\HotTemp\stateinfo\current_connect_info`
/// while connected to a match (it holds a `battle_id`) and clears it afterwards. Returns the
/// battle id when the contents describe a match.
pub fn marvel_battle_id(contents: &str) -> Option<String> {
    let text = contents.trim();
    if text.is_empty() {
        return None;
    }
    // JSON (any nesting) first, then a loose `battle_id = value` scan.
    fn find(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::Object(m) => m.iter().find_map(|(k, v)| {
                if k.to_ascii_lowercase().contains("battle") {
                    let s = match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        _ => String::new(),
                    };
                    (!s.is_empty() && s != "0").then_some(s)
                } else {
                    find(v)
                }
            }),
            serde_json::Value::Array(a) => a.iter().find_map(find),
            _ => None,
        }
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        return find(&v);
    }
    let lower = text.to_ascii_lowercase();
    let i = lower.find("battle_id").or_else(|| lower.find("battleid"))?;
    let rest = &text[i..];
    let value: String = rest
        .split_once([':', '='])?
        .1
        .trim_start_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!value.is_empty() && value != "0").then_some(value)
}

#[cfg(test)]
mod marvel_tests {
    use super::marvel_battle_id;

    #[test]
    fn battle_id_formats() {
        assert_eq!(
            marvel_battle_id(r#"{"battle_id":"1234567","map":"Tokyo"}"#).as_deref(),
            Some("1234567")
        );
        assert_eq!(marvel_battle_id(r#"{"info":{"BattleId":99}}"#).as_deref(), Some("99"));
        assert_eq!(marvel_battle_id("battle_id=abc-12\nserver=x").as_deref(), Some("abc-12"));
        assert_eq!(marvel_battle_id(r#"{"battle_id":""}"#), None);
        assert_eq!(marvel_battle_id("   "), None);
        assert_eq!(marvel_battle_id(r#"{"player_uid":"5"}"#), None);
    }
}
