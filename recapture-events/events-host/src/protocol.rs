//! Messages to the Recapture engine on `\\.\pipe\recapture-events`: one JSON object per line.
//!
//! ```json
//! {"v":1,"type":"hello","game":"cs2","pid":1234}
//! {"v":1,"type":"ping"}
//! {"v":1,"type":"event","kind":"kill","unixMs":1761400000123,"source":"api:cs2-gsi","meta":{}}
//! {"v":1,"type":"clip","startUnixMs":1761399994123,"endUnixMs":1761400003123,"title":"Kill","kinds":["kill"],"auto":true}
//! ```
//!
//! This is the whole contract between the GPL events exe and Recapture: nothing else is shared.

use recap_core::events::GameEvent;
use serde::Serialize;

pub const VERSION: u32 = 1;
pub const PIPE_NAME: &str = r"\\.\pipe\recapture-events";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Message {
    Hello { game: String, pid: u32 },
    /// Every few seconds when nothing else is sent, so a closed pipe is noticed.
    Ping,
    Event {
        kind: String,
        unix_ms: u64,
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        confidence: Option<f32>,
        #[serde(skip_serializing_if = "serde_json::Value::is_null")]
        meta: serde_json::Value,
    },
    /// `auto`: an event inside asked for a clip (its action is `clip`); the engine's setting decides either way.
    Clip { start_unix_ms: u64, end_unix_ms: u64, title: String, kinds: Vec<String>, auto: bool },
}

impl Message {
    /// `ev.t_ms` is unix ms.
    pub fn event(ev: &GameEvent) -> Self {
        Message::Event {
            kind: ev.kind.clone(),
            unix_ms: ev.t_ms,
            source: ev.source.label(),
            confidence: ev.confidence,
            meta: ev.meta.clone(),
        }
    }

    pub fn to_line(&self) -> String {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            o.insert("v".into(), VERSION.into());
        }
        let mut s = v.to_string();
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_match_the_documented_shape() {
        let line = Message::Clip { start_unix_ms: 1, end_unix_ms: 2, title: "Kill".into(), kinds: vec!["kill".into()], auto: true }.to_line();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["type"], "clip");
        assert_eq!(v["startUnixMs"], 1);
        assert!(line.ends_with('\n'));
    }
}
