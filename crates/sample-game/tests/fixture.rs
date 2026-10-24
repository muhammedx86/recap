//! The exported fixture loads in Recap as a ready game with round markers.

use recap_core::games::load_games;
use recap_sample_game::{export_fixture, ALL_BANNERS, GAME_ID, HEIGHT, PROCESS_NAME};

#[test]
fn fixture_loads_with_six_ready_detectors() {
    let tmp = tempfile::tempdir().unwrap();
    let game_dir = export_fixture(tmp.path()).unwrap();
    assert!(game_dir.join("game.json").exists());

    let games = load_games(&[tmp.path().to_path_buf()]);
    assert_eq!(games.len(), 1);
    let g = &games[0];
    assert_eq!(g.def.id, GAME_ID);
    assert!(g.matches_process(PROCESS_NAME));
    assert!(g.has_rounds());
    assert_eq!(g.ready_detectors().count(), 6);
    for b in ALL_BANNERS {
        let d = g.detectors.iter().find(|d| d.def.id == b.detector_id()).expect("detector");
        assert_eq!(d.def.event, b.kind());
        let tpl = &d.template_paths()[0];
        let img = image::open(tpl).unwrap();
        let (_, _, w, h) = b.rect();
        assert_eq!((img.width(), img.height()), (w as u32, h as u32), "{}", b.detector_id());
        assert_eq!(d.def.template_frame_height, HEIGHT as u32);
    }
}
