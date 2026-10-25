//! Every bundled game/detector file must load and validate.

use std::path::PathBuf;

use recap_core::{
    events::{builtin_event_types, ICONS},
    games::load_games,
};

#[test]
fn bundled_games_load() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games");
    let files = std::fs::read_dir(&root).unwrap().flatten().filter(|e| e.path().is_dir()).count();
    let games = load_games(std::slice::from_ref(&root));
    assert_eq!(games.len(), files, "a game folder failed to load");
    for g in &games {
        let detector_files = std::fs::read_dir(g.dir.join("detectors")).map(|d| d.count()).unwrap_or(0);
        assert_eq!(
            g.detectors.len(),
            detector_files,
            "{}: a detector file failed to load or validate",
            g.def.id
        );
        let types = g.event_types();
        // Blockers mark nothing, so their `event` is not an event type.
        for d in g.detectors.iter().filter(|d| d.def.blocks.is_empty()) {
            assert!(
                types.iter().any(|t| t.id == d.def.event),
                "{}/{} emits unknown event type {}",
                g.def.id,
                d.def.id,
                d.def.event
            );
        }
        for t in &g.def.event_types {
            t.validate().unwrap();
            assert!(
                !builtin_event_types().iter().any(|b| b.id == t.id),
                "{} redefines built-in {}",
                g.def.id,
                t.id
            );
        }
    }
    let ids: Vec<_> = games.iter().map(|g| g.def.id.as_str()).collect();
    for want in ["cs2", "league-of-legends", "marvel-rivals", "r6-siege", "valorant"] {
        assert!(ids.contains(&want), "missing {want}");
    }
    assert!(games.iter().find(|g| g.def.id == "r6-siege").unwrap().has_rounds());
    assert!(games.iter().find(|g| g.def.id == "cs2").unwrap().has_rounds());
    assert_eq!(ICONS.len(), 15);
}
