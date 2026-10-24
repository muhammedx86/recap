//! The end-of-match screen: map name, both scores, VICTORY/DEFEAT and three player cards (name,
//! character icon, K/D/A), with the local player's name in yellow. Text is drawn with a system
//! font when one is found (Segoe UI / Arial on Windows, DejaVu on Linux), otherwise with a
//! built-in 5x7 pixel font. Recap's screen readers read it in the E2E test.

use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};

use recap_core::games::{ChoiceOption, Highlight, ReadWhen, ReaderDef, ReaderKind, Region, Slots};

use crate::{fill_rect, rgb, HEIGHT, WIDTH};

/// Character icons the cards can show (choice reader values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Ring,
    Cross,
}

impl Icon {
    pub fn name(self) -> &'static str {
        match self {
            Icon::Ring => "Ring",
            Icon::Cross => "Cross",
        }
    }
}

pub struct Card {
    pub name: &'static str,
    pub icon: Icon,
    pub kda: (u32, u32, u32),
    pub me: bool,
}

/// What one match's end screen shows.
pub struct EndScreen {
    pub t_s: f64,
    pub duration_s: f64,
    pub map: &'static str,
    pub team_score: u32,
    pub enemy_score: u32,
    pub cards: [Card; 3],
}

impl EndScreen {
    pub fn me(&self) -> &Card {
        self.cards.iter().find(|c| c.me).expect("one card is the player")
    }
    pub fn result(&self) -> &'static str {
        if self.team_score > self.enemy_score {
            "VICTORY"
        } else {
            "DEFEAT"
        }
    }
}

pub fn end_screens() -> [EndScreen; 2] {
    let card = |name, icon, kda, me| Card { name, icon, kda, me };
    [
        EndScreen {
            t_s: 31.3,
            duration_s: 6.0,
            map: "HARBOR",
            team_score: 13,
            enemy_score: 11,
            cards: [
                card("ALPHA", Icon::Cross, (0, 2, 1), false),
                card("YOU", Icon::Ring, (3, 1, 2), true),
                card("BRAVO", Icon::Cross, (5, 2, 0), false),
            ],
        },
        EndScreen {
            t_s: 58.2,
            duration_s: 5.0,
            map: "CANYON",
            team_score: 9,
            enemy_score: 13,
            cards: [
                card("ALPHA", Icon::Ring, (2, 3, 1), false),
                card("BRAVO", Icon::Ring, (4, 1, 0), false),
                card("YOU", Icon::Cross, (1, 0, 4), true),
            ],
        },
    ]
}

pub const YELLOW: (i32, i32, i32) = (226, 232, 143);
const WHITE: (i32, i32, i32) = (228, 228, 228);
const CARD_X0: usize = 200;
const CARD_STEP: usize = 300;
const CARD_Y: usize = 250;
const CARD_W: usize = 260;
const CARD_H: usize = 290;

fn glyph(c: char) -> [&'static str; 7] {
    match c {
        '0' => ["01110", "10001", "10011", "10101", "11001", "10001", "01110"],
        '1' => ["00100", "01100", "00100", "00100", "00100", "00100", "01110"],
        '2' => ["01110", "10001", "00001", "00010", "00100", "01000", "11111"],
        '3' => ["11110", "00001", "00001", "01110", "00001", "00001", "11110"],
        '4' => ["00010", "00110", "01010", "10010", "11111", "00010", "00010"],
        '5' => ["11111", "10000", "11110", "00001", "00001", "10001", "01110"],
        '6' => ["00110", "01000", "10000", "11110", "10001", "10001", "01110"],
        '7' => ["11111", "00001", "00010", "00100", "01000", "01000", "01000"],
        '8' => ["01110", "10001", "10001", "01110", "10001", "10001", "01110"],
        '9' => ["01110", "10001", "10001", "01111", "00001", "00010", "01100"],
        'A' => ["01110", "10001", "10001", "11111", "10001", "10001", "10001"],
        'B' => ["11110", "10001", "10001", "11110", "10001", "10001", "11110"],
        'C' => ["01110", "10001", "10000", "10000", "10000", "10001", "01110"],
        'D' => ["11110", "10001", "10001", "10001", "10001", "10001", "11110"],
        'E' => ["11111", "10000", "10000", "11110", "10000", "10000", "11111"],
        'F' => ["11111", "10000", "10000", "11110", "10000", "10000", "10000"],
        'H' => ["10001", "10001", "10001", "11111", "10001", "10001", "10001"],
        'I' => ["01110", "00100", "00100", "00100", "00100", "00100", "01110"],
        'L' => ["10000", "10000", "10000", "10000", "10000", "10000", "11111"],
        'N' => ["10001", "11001", "10101", "10011", "10001", "10001", "10001"],
        'O' => ["01110", "10001", "10001", "10001", "10001", "10001", "01110"],
        'P' => ["11110", "10001", "10001", "11110", "10000", "10000", "10000"],
        'R' => ["11110", "10001", "10001", "11110", "10100", "10010", "10001"],
        'S' => ["01111", "10000", "10000", "01110", "00001", "00001", "11110"],
        'T' => ["11111", "00100", "00100", "00100", "00100", "00100", "00100"],
        'U' => ["10001", "10001", "10001", "10001", "10001", "10001", "01110"],
        'V' => ["10001", "10001", "10001", "10001", "10001", "01010", "00100"],
        'Y' => ["10001", "10001", "01010", "00100", "00100", "00100", "00100"],
        '/' => ["00001", "00010", "00010", "00100", "01000", "01000", "10000"],
        _ => ["00000"; 7],
    }
}

fn system_font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        [
            r"C:\Windows\Fonts\segoeuib.ttf",
            r"C:\Windows\Fonts\arialbd.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        ]
        .iter()
        .find_map(|p| std::fs::read(p).ok().and_then(|b| FontVec::try_from_vec(b).ok()))
    })
    .as_ref()
}

/// Draw `text` with its top-left at (x, y). `scale` is the pixel-font scale; with a system font
/// the cap height matches the pixel font's (7 * scale).
pub fn draw_text(buf: &mut [u32], text: &str, x: usize, y: usize, scale: usize, color: (i32, i32, i32)) {
    match system_font() {
        Some(f) => draw_text_ttf(buf, f, text, x, y, scale, color),
        None => draw_text_pixel(buf, text, x, y, scale, color),
    }
}

fn draw_text_ttf(buf: &mut [u32], font: &FontVec, text: &str, x: usize, y: usize, scale: usize, color: (i32, i32, i32)) {
    // Size the font so capitals are 7*scale px tall.
    let cap = 7.0 * scale as f32;
    let probe = font.as_scaled(PxScale::from(100.0));
    let cap_ratio = probe
        .outline_glyph(probe.scaled_glyph('H'))
        .map(|g| g.px_bounds().height() / 100.0)
        .unwrap_or(0.7);
    let sf = font.as_scaled(PxScale::from(cap / cap_ratio.max(0.3)));
    let baseline = y as f32 + cap;
    let mut pen = x as f32;
    let mut prev = None;
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            pen += sf.kern(p, id);
        }
        let g = id.with_scale_and_position(sf.scale(), ab_glyph::point(pen, baseline));
        pen += sf.h_advance(id) + scale as f32 * 0.6;
        prev = Some(id);
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64);
                if px < 0 || py < 0 || px >= WIDTH as i64 || py >= HEIGHT as i64 || cov <= 0.0 {
                    return;
                }
                let i = py as usize * WIDTH + px as usize;
                let bg = buf[i];
                let mix = |c: i32, shift: u32| {
                    let b = ((bg >> shift) & 0xff) as f32;
                    (b + (c as f32 - b) * cov.min(1.0)) as i32
                };
                buf[i] = rgb(mix(color.0, 16), mix(color.1, 8), mix(color.2, 0));
            });
        }
    }
}

/// Draw `text` with the built-in 5x7 font, each font pixel `scale` px.
fn draw_text_pixel(buf: &mut [u32], text: &str, x: usize, y: usize, scale: usize, color: (i32, i32, i32)) {
    let mut cx = x;
    for ch in text.chars() {
        if ch != ' ' {
            for (row, bits) in glyph(ch).iter().enumerate() {
                for (col, b) in bits.chars().enumerate() {
                    if b == '1' {
                        fill_rect(
                            buf,
                            (cx + col * scale) as i64,
                            (y + row * scale) as i64,
                            scale as i64,
                            scale as i64,
                            rgb(color.0, color.1, color.2),
                        );
                    }
                }
            }
        }
        cx += 6 * scale;
    }
}

/// Draw a character icon in an 80x80 box at (x, y).
pub fn draw_icon(buf: &mut [u32], icon: Icon, x: usize, y: usize) {
    for dy in 0..80usize {
        for dx in 0..80usize {
            let (fx, fy) = (dx as f32 - 40.0, dy as f32 - 40.0);
            let on = match icon {
                Icon::Ring => ((fx * fx + fy * fy).sqrt() - 26.0).abs() <= 6.0,
                Icon::Cross => (fx.abs() <= 7.0 || fy.abs() <= 7.0) && fx.abs() <= 30.0 && fy.abs() <= 30.0,
            };
            let v = if on { 220 } else { 40 };
            buf[(y + dy) * WIDTH + x + dx] = rgb(v, v, v + 10);
        }
    }
}

fn card_x(i: usize) -> usize {
    CARD_X0 + i * CARD_STEP
}

pub fn draw_end_screen(buf: &mut [u32], s: &EndScreen) {
    fill_rect(buf, 0, 0, WIDTH as i64, HEIGHT as i64, rgb(20, 24, 34));
    draw_text(buf, s.map, 40, 30, 4, WHITE);
    draw_text(buf, &s.team_score.to_string(), 40, 100, 8, (124, 195, 163));
    let es = s.enemy_score.to_string();
    draw_text(buf, &es, WIDTH - 40 - es.len() * 48 + 8, 100, 8, (214, 139, 128));
    let r = s.result();
    draw_text(buf, r, (WIDTH - r.len() * 48) / 2, 60, 8, WHITE);
    for (i, c) in s.cards.iter().enumerate() {
        let x = card_x(i);
        fill_rect(buf, x as i64, CARD_Y as i64, CARD_W as i64, CARD_H as i64, rgb(36, 42, 56));
        draw_text(buf, c.name, x + 20, CARD_Y + 20, 4, if c.me { YELLOW } else { WHITE });
        draw_icon(buf, c.icon, x + 90, CARD_Y + 80);
        let kda = format!("{} / {} / {}", c.kda.0, c.kda.1, c.kda.2);
        draw_text(buf, &kda, x + 20, CARD_Y + 200, 4, WHITE);
    }
}

fn frac(x: usize, y: usize, w: usize, h: usize) -> Region {
    Region {
        x: x as f32 / WIDTH as f32,
        y: y as f32 / HEIGHT as f32,
        w: w as f32 / WIDTH as f32,
        h: h as f32 / HEIGHT as f32,
    }
}

fn reader(id: &str, label: &str, field: &str, kind: ReaderKind, region: Region) -> ReaderDef {
    ReaderDef {
        id: id.into(),
        label: label.into(),
        field: field.into(),
        kind,
        region,
        slots: None,
        highlight: None,
        when: ReadWhen::End,
        pick: Default::default(),
        sample_fps: 2.0,
        pattern: None,
        values: vec![],
        options: vec![],
        threshold: 0.8,
        method: Default::default(),
        template_frame_height: HEIGHT as u32,
        enabled: true,
        notes: Some("Sample game fixture for Recap's end-to-end test.".into()),
    }
}

/// The local player's card: highlighted by the yellow name (relative to a slot box at y0, height h).
fn highlight(slot_y: usize, slot_h: usize) -> Highlight {
    let name_y = CARD_Y + 20;
    Highlight {
        area: Region {
            x: 0.05,
            y: (name_y as f32 - slot_y as f32) / slot_h as f32,
            w: 0.6,
            h: 28.0 / slot_h as f32,
        },
        color: format!("#{:02x}{:02x}{:02x}", YELLOW.0, YELLOW.1, YELLOW.2),
        tolerance: 40,
    }
}

/// Readers for the fixture. Choice references are written by `export_fixture`.
pub fn readers() -> Vec<ReaderDef> {
    let slots = Some(Slots {
        count: 3,
        step_x: CARD_STEP as f32 / WIDTH as f32,
        step_y: 0.0,
    });
    let mut map = reader("map", "Map", "map", ReaderKind::Text, frac(30, 22, 240, 44));
    map.values = vec!["Harbor".into(), "Canyon".into(), "Summit".into()];
    let mut team = reader(
        "team-score",
        "Your team score",
        "team_score",
        ReaderKind::Text,
        frac(28, 88, 140, 80),
    );
    team.pattern = Some(r"^\s*(\d{1,2})\s*$".into());
    let mut enemy = reader(
        "enemy-score",
        "Enemy score",
        "enemy_score",
        ReaderKind::Text,
        frac(1110, 88, 150, 80),
    );
    enemy.pattern = team.pattern.clone();
    let result = reader("result", "Result", "result", ReaderKind::Text, frac(420, 50, 440, 76));
    let (ky, kh) = (CARD_Y + 190, 48);
    let mut kda = reader("kda", "Your K/D/A", "kda", ReaderKind::Text, frac(CARD_X0 + 10, ky, 240, kh));
    kda.slots = slots;
    kda.highlight = Some(highlight(ky, kh));
    kda.pattern = Some(r"(?P<kills>\d+)\D{1,4}(?P<deaths>\d+)\D{1,4}(?P<assists>\d+)".into());
    let (iy, ih) = (CARD_Y + 70, 100);
    let mut ch = reader(
        "character",
        "Your character",
        "character",
        ReaderKind::Choice,
        frac(CARD_X0 + 80, iy, 100, ih),
    );
    ch.slots = slots;
    ch.highlight = Some(highlight(iy, ih));
    ch.options = [Icon::Ring, Icon::Cross]
        .iter()
        .map(|i| ChoiceOption {
            value: i.name().into(),
            templates: vec![format!("templates/icon-{}.png", i.name().to_lowercase())],
        })
        .collect();
    vec![map, team, enemy, result, kda, ch]
}

/// Icon reference image (80x80) as the fixture stores it.
pub fn icon_image(icon: Icon) -> image::RgbImage {
    let mut buf = vec![0u32; WIDTH * HEIGHT];
    draw_icon(&mut buf, icon, 0, 0);
    image::RgbImage::from_fn(80, 80, |x, y| {
        let p = buf[y as usize * WIDTH + x as usize];
        image::Rgb([(p >> 16) as u8, (p >> 8) as u8, p as u8])
    })
}
