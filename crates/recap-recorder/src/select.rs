//! Pure decision helpers: which encoders to try (and at what size), their settings, and which
//! window to capture.

use recap_core::protocol::{EncoderMemory, QualityPreset, RateControl, VideoConfig};

/// OBS encoder ids, most preferred first, for each family.
const NVENC: &[&str] = &["obs_nvenc_h264_tex", "jim_nvenc", "obs_nvenc_h264_soft", "ffmpeg_nvenc"];
const QSV: &[&str] = &["obs_qsv11_v2", "obs_qsv11", "obs_qsv11_soft_v2", "obs_qsv11_soft"];
const AMF: &[&str] = &["h264_texture_amf"];
pub const X264: &str = "obs_x264";

/// Hardware H.264 encoders top out at 4096 px a side; above this the output is scaled down
/// to fit [`HW_MAX_W`] x [`HW_MAX_H`].
const HW_LIMIT: u32 = 4000;
const HW_MAX_W: u32 = 3840;
const HW_MAX_H: u32 = 2160;
/// When x264 is a fallback (no GPU encoder started), keep the CPU load low: 720p, at most 60 fps.
pub const CPU_FALLBACK_HEIGHT: u32 = 720;
pub const CPU_FALLBACK_FPS: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Nvenc,
    Qsv,
    Amf,
    X264,
}

impl Family {
    pub fn of(id: &str) -> Family {
        if NVENC.contains(&id) {
            Family::Nvenc
        } else if QSV.contains(&id) {
            Family::Qsv
        } else if AMF.contains(&id) {
            Family::Amf
        } else {
            Family::X264
        }
    }

    fn ids(self) -> &'static [&'static str] {
        match self {
            Family::Nvenc => NVENC,
            Family::Qsv => QSV,
            Family::Amf => AMF,
            Family::X264 => &[X264],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Family::Nvenc => "NVENC",
            Family::Qsv => "Quick Sync",
            Family::Amf => "AMF",
            Family::X264 => "x264 (CPU)",
        }
    }

    fn parse(requested: &str) -> Option<Family> {
        match requested.to_ascii_lowercase().as_str() {
            "nvenc" => Some(Family::Nvenc),
            "qsv" => Some(Family::Qsv),
            "amf" => Some(Family::Amf),
            "x264" => Some(Family::X264),
            _ => None,
        }
    }
}

/// Auto order. AMF goes after Quick Sync: AMD iGPUs list AMF but often cannot keep up.
const AUTO_ORDER: [Family; 3] = [Family::Nvenc, Family::Qsv, Family::Amf];

/// One encoder to try, with the canvas/output size and frame rate to use with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeAttempt {
    pub encoder: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// x264 used because no GPU encoder could (capped to 720p / 60 fps).
    pub cpu_fallback: bool,
}

/// Encoder ids from `memory.failed`, if the memory was written with the same encoder list.
pub fn remembered_failures(memory: Option<&EncoderMemory>, available: &[String]) -> Vec<String> {
    match memory {
        Some(m) if same_set(&m.available, available) => m.failed.clone(),
        _ => vec![],
    }
}

fn same_set(a: &[String], b: &[String]) -> bool {
    let mut a: Vec<&String> = a.iter().collect();
    let mut b: Vec<&String> = b.iter().collect();
    a.sort();
    a.dedup();
    b.sort();
    b.dedup();
    a == b
}

/// Memory to report after a start: today's list, failures still valid plus new ones, and the
/// encoder that started.
pub fn updated_memory(previous: Option<&EncoderMemory>, available: &[String], failed_now: &[String], working: Option<&str>) -> EncoderMemory {
    let mut failed = remembered_failures(previous, available);
    for f in failed_now {
        if !failed.contains(f) {
            failed.push(f.clone());
        }
    }
    if let Some(w) = working {
        failed.retain(|f| f != w);
    }
    EncoderMemory {
        available: available.to_vec(),
        failed,
        working: working.map(str::to_string).or_else(|| previous.and_then(|p| p.working.clone())),
    }
}

/// The encoders to try, in order, each with its size. The first that starts records.
///
/// * auto: NVENC, Quick Sync, AMF, then x264 as a CPU fallback (720p, <= 60 fps). Encoders in
///   `skip` (remembered failures) are left out, unless they are all there is.
/// * a named GPU family is tried first, then auto's order.
/// * "x264": only x264, at the requested size.
///
/// GPU encoders get outputs above 4000 px scaled down to fit 3840x2160 (H.264 hardware
/// limits); the canvas is always the output size, so OBS does one scaling pass.
pub fn plan_encoders(
    requested: &str,
    available: &[String],
    skip: &[String],
    canvas: (u32, u32),
    requested_height: Option<u32>,
    fps: u32,
) -> Vec<EncodeAttempt> {
    let fps = fps.clamp(10, 240);
    let (w, h) = output_size(canvas.0, canvas.1, requested_height);
    let req = Family::parse(requested);
    if req == Some(Family::X264) {
        return vec![EncodeAttempt {
            encoder: X264.into(),
            width: w,
            height: h,
            fps,
            cpu_fallback: false,
        }];
    }
    let has = |id: &str| available.iter().any(|a| a == id);
    let mut families: Vec<Family> = req.into_iter().collect();
    families.extend(AUTO_ORDER.iter().copied().filter(|f| Some(*f) != req));
    let (hw_w, hw_h) = fit_hw(w, h);
    let mut out: Vec<EncodeAttempt> = Vec::new();
    let mut skipped: Vec<EncodeAttempt> = Vec::new();
    for f in families {
        // The best id of the family that OBS lists (one per family: the others are the same
        // hardware through an older path).
        let Some(id) = f.ids().iter().find(|id| has(id)) else { continue };
        let a = EncodeAttempt {
            encoder: id.to_string(),
            width: hw_w,
            height: hw_h,
            fps,
            cpu_fallback: false,
        };
        // An explicitly requested family is always tried.
        if skip.iter().any(|s| s == id) && Some(f) != req {
            skipped.push(a);
        } else {
            out.push(a);
        }
    }
    let cap_h = requested_height.map_or(CPU_FALLBACK_HEIGHT, |r| r.min(CPU_FALLBACK_HEIGHT));
    let (cw, ch) = output_size(canvas.0, canvas.1, Some(cap_h));
    out.push(EncodeAttempt {
        encoder: X264.into(),
        width: cw,
        height: ch,
        fps: fps.min(CPU_FALLBACK_FPS),
        cpu_fallback: true,
    });
    // Remembered failures come back only after x264 fails too.
    out.extend(skipped);
    out
}

/// Scale (w, h) down to fit the hardware limits when a side reaches 4000 px.
fn fit_hw(w: u32, h: u32) -> (u32, u32) {
    if w < HW_LIMIT && h < HW_LIMIT {
        return (w, h);
    }
    let s = (HW_MAX_W as f64 / w as f64).min(HW_MAX_H as f64 / h as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32).max(2)) & !1;
    (even(w as f64 * s), even(h as f64 * s))
}

/// Backwards-compatible single pick (first attempt of the plan).
pub fn choose_encoder(requested: &str, available: &[String]) -> String {
    plan_encoders(requested, available, &[], (1920, 1080), None, 60)
        .into_iter()
        .next()
        .map(|a| a.encoder)
        .unwrap_or_else(|| X264.into())
}

/// True for encoder ids that run on the GPU.
pub fn is_hardware(id: &str) -> bool {
    Family::of(id) != Family::X264
}

/// A setting value for an OBS encoder.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Str(&'static str),
    Int(i64),
    Bool(bool),
}

/// Constant-quality level per preset: (GPU CQP, x264 CRF). Lower is better and bigger.
pub fn quality_level(q: QualityPreset) -> (i64, i64) {
    match q {
        QualityPreset::High => (20, 20),
        QualityPreset::Medium => (24, 23),
        QualityPreset::Low => (28, 27),
    }
}

/// Encoder settings for `encoder`. Tuned for low overhead on the game: balanced presets, no
/// look-ahead, no psycho-visual tuning or multipass (which cost GPU time), 2 B-frames where
/// the encoder supports them, and 1 s keyframes so stream-copy clip cuts land close to the
/// requested time.
pub fn encoder_settings(encoder: &str, v: &VideoConfig) -> Vec<(&'static str, SettingValue)> {
    use SettingValue::*;
    let quality = v.rate_control == RateControl::Quality;
    let (cqp, crf) = quality_level(v.quality);
    let bitrate = v.bitrate_kbps.clamp(1000, 150_000) as i64;
    let mut s: Vec<(&'static str, SettingValue)> = vec![("keyint_sec", Int(1))];
    match Family::of(encoder) {
        Family::Nvenc => {
            s.push(("rate_control", Str(if quality { "CQP" } else { "CBR" })));
            if quality {
                s.push(("cqp", Int(cqp)));
            } else {
                s.push(("bitrate", Int(bitrate)));
            }
            s.extend([
                ("preset", Str("p4")),
                ("tune", Str("hq")),
                ("multipass", Str("disabled")),
                ("lookahead", Bool(false)),
                ("adaptive_quantization", Bool(false)),
                ("bf", Int(2)),
                ("profile", Str("high")),
            ]);
        }
        Family::Qsv => {
            s.push(("rate_control", Str(if quality { "CQP" } else { "CBR" })));
            if quality {
                // "cqp" on current plugins; qpi/qpp/qpb on older ones.
                s.extend([("cqp", Int(cqp)), ("qpi", Int(cqp)), ("qpp", Int(cqp)), ("qpb", Int(cqp))]);
            } else {
                s.push(("bitrate", Int(bitrate)));
            }
            s.extend([
                ("target_usage", Str("TU4")),
                ("latency", Str("normal")),
                ("bframes", Int(2)),
                ("profile", Str("high")),
            ]);
        }
        Family::Amf => {
            s.push(("rate_control", Str(if quality { "CQP" } else { "CBR" })));
            if quality {
                s.push(("cqp", Int(cqp)));
            } else {
                s.push(("bitrate", Int(bitrate)));
            }
            s.extend([
                ("preset", Str(if v.quality == QualityPreset::Low { "speed" } else { "balanced" })),
                ("pre_analysis", Bool(false)),
                ("profile", Str("high")),
            ]);
        }
        Family::X264 => {
            s.push(("rate_control", Str(if quality { "CRF" } else { "CBR" })));
            if quality {
                s.push(("crf", Int(crf)));
            } else {
                s.push(("bitrate", Int(bitrate)));
            }
            s.extend([("preset", Str("veryfast")), ("profile", Str("high"))]);
        }
    }
    s
}

/// Human description of a fallback, for the app's status line.
pub fn fallback_message(first: &EncodeAttempt, used: &EncodeAttempt, failed: &[String]) -> Option<String> {
    if used.encoder == first.encoder && failed.is_empty() && !used.cpu_fallback {
        return None;
    }
    let tried: Vec<&str> = failed.iter().map(|f| Family::of(f).label()).collect();
    let why = if tried.is_empty() {
        "no GPU encoder is available".to_string()
    } else {
        format!("{} did not start", tried.join(", "))
    };
    Some(if used.cpu_fallback {
        format!(
            "Recording on the CPU (x264) at {}p{} because {why}; this can cost FPS in game",
            used.height, used.fps
        )
    } else {
        format!("Recording with {} because {why}", Family::of(&used.encoder).label())
    })
}

/// The bits of a window we need to decide whether it belongs to the target game.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowCandidate {
    /// Full path of the window's executable.
    pub full_exe: String,
    pub pid: u32,
    pub is_game: bool,
    pub has_title: bool,
}

/// File name of a Windows or Unix path.
pub fn exe_file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Index of the best window for the target process, if any.
/// A window qualifies if its pid matches `pid`, or its exe file name matches `process`
/// (case-insensitive). Among qualifying windows, pid matches, game windows and titled
/// windows win, in that order.
pub fn pick_window(cands: &[WindowCandidate], process: Option<&str>, pid: Option<u32>) -> Option<usize> {
    cands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let pid_match = pid.is_some_and(|p| p == c.pid);
            let name_match = process.is_some_and(|p| exe_file_name(&c.full_exe).eq_ignore_ascii_case(exe_file_name(p)));
            (pid_match || name_match).then_some((i, (pid_match, c.is_game, c.has_title)))
        })
        .max_by_key(|(i, key)| (*key, std::cmp::Reverse(*i)))
        .map(|(i, _)| i)
}

/// Output size for a requested height, keeping the base aspect ratio. Never upscales,
/// and keeps both sides even (encoders require it).
pub fn output_size(base_w: u32, base_h: u32, requested_h: Option<u32>) -> (u32, u32) {
    let even = |v: u32| (v.max(2)) & !1;
    match requested_h {
        Some(h) if h > 0 && h < base_h => {
            let w = (base_w as u64 * h as u64 + base_h as u64 / 2) / base_h as u64;
            (even(w as u32), even(h))
        }
        _ => (even(base_w), even(base_h)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn encoder_auto_prefers_hardware() {
        assert_eq!(
            choose_encoder("auto", &s(&["obs_x264", "h264_texture_amf", "obs_nvenc_h264_tex"])),
            "obs_nvenc_h264_tex"
        );
        // Quick Sync beats AMF (AMD iGPUs list AMF but are slow).
        assert_eq!(choose_encoder("auto", &s(&["obs_x264", "h264_texture_amf", "obs_qsv11_v2"])), "obs_qsv11_v2");
        assert_eq!(choose_encoder("auto", &s(&["obs_x264"])), "obs_x264");
        assert_eq!(choose_encoder("auto", &[]), "obs_x264");
    }

    #[test]
    fn encoder_specific_and_fallbacks() {
        let all = s(&["obs_x264", "h264_texture_amf", "obs_nvenc_h264_tex", "obs_qsv11_v2"]);
        assert_eq!(choose_encoder("qsv", &all), "obs_qsv11_v2");
        assert_eq!(choose_encoder("AMF", &all), "h264_texture_amf");
        assert_eq!(choose_encoder("x264", &all), "obs_x264");
        // Requested family missing -> auto.
        assert_eq!(choose_encoder("nvenc", &s(&["obs_x264", "h264_texture_amf"])), "h264_texture_amf");
        assert_eq!(choose_encoder("bogus", &s(&["obs_x264"])), "obs_x264");
        assert!(!is_hardware("obs_x264"));
        assert!(is_hardware("h264_texture_amf"));
        assert!(is_hardware("jim_nvenc"));
    }

    fn ids(p: &[EncodeAttempt]) -> Vec<&str> {
        p.iter().map(|a| a.encoder.as_str()).collect()
    }

    #[test]
    fn plan_order_sizes_and_cpu_cap() {
        let all = s(&["obs_x264", "h264_texture_amf", "obs_nvenc_h264_tex", "jim_nvenc", "obs_qsv11_v2"]);
        let p = plan_encoders("auto", &all, &[], (2560, 1440), None, 144);
        assert_eq!(ids(&p), ["obs_nvenc_h264_tex", "obs_qsv11_v2", "h264_texture_amf", "obs_x264"]);
        assert_eq!((p[0].width, p[0].height, p[0].fps), (2560, 1440, 144));
        // x264 as a fallback: 720p, 60 fps at most.
        let x = p.last().unwrap();
        assert!(x.cpu_fallback);
        assert_eq!((x.width, x.height, x.fps), (1280, 720, 60));
        // A lower requested height is kept for x264 too.
        let p = plan_encoders("auto", &s(&["obs_x264"]), &[], (1920, 1080), Some(480), 30);
        assert_eq!((p[0].width, p[0].height, p[0].fps, p[0].cpu_fallback), (852, 480, 30, true));
        // Named family first, then auto order.
        let p = plan_encoders("amf", &all, &[], (1920, 1080), None, 60);
        assert_eq!(ids(&p), ["h264_texture_amf", "obs_nvenc_h264_tex", "obs_qsv11_v2", "obs_x264"]);
        // Explicit x264: only x264, full size, not capped.
        let p = plan_encoders("x264", &all, &[], (2560, 1440), None, 120);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].width, p[0].height, p[0].fps, p[0].cpu_fallback), (2560, 1440, 120, false));
    }

    #[test]
    fn plan_skips_remembered_failures_and_fits_hw_limits() {
        let all = s(&["obs_x264", "obs_nvenc_h264_tex", "obs_qsv11_v2"]);
        let skip = s(&["obs_nvenc_h264_tex"]);
        let p = plan_encoders("auto", &all, &skip, (1920, 1080), None, 60);
        assert_eq!(ids(&p), ["obs_qsv11_v2", "obs_x264", "obs_nvenc_h264_tex"]);
        // ...but an explicitly chosen family is still tried first.
        let p = plan_encoders("nvenc", &all, &skip, (1920, 1080), None, 60);
        assert_eq!(p[0].encoder, "obs_nvenc_h264_tex");
        // 5120x1440 super-ultrawide: GPU encoders get 3840x1080, x264 720p.
        let p = plan_encoders("auto", &all, &[], (5120, 1440), None, 60);
        assert_eq!((p[0].width, p[0].height), (3840, 1080));
        assert_eq!((p[2].width, p[2].height), (2560, 720));
        // 4K 16:9 is under the limit.
        let p = plan_encoders("auto", &all, &[], (3840, 2160), None, 60);
        assert_eq!((p[0].width, p[0].height), (3840, 2160));
    }

    #[test]
    fn memory_is_trusted_only_for_the_same_encoder_list() {
        let avail = s(&["obs_x264", "obs_nvenc_h264_tex"]);
        let mem = EncoderMemory {
            available: s(&["obs_nvenc_h264_tex", "obs_x264"]),
            failed: s(&["obs_nvenc_h264_tex"]),
            working: Some("obs_x264".into()),
        };
        assert_eq!(remembered_failures(Some(&mem), &avail), s(&["obs_nvenc_h264_tex"]));
        // New GPU: list changed, try everything again.
        let new_gpu = s(&["obs_x264", "obs_nvenc_h264_tex", "obs_qsv11_v2"]);
        assert!(remembered_failures(Some(&mem), &new_gpu).is_empty());
        let m = updated_memory(Some(&mem), &new_gpu, &s(&["obs_qsv11_v2"]), Some("obs_nvenc_h264_tex"));
        assert_eq!(m.failed, s(&["obs_qsv11_v2"]));
        assert_eq!(m.working.as_deref(), Some("obs_nvenc_h264_tex"));
        // A remembered failure that now works is forgotten.
        let m = updated_memory(Some(&mem), &avail, &[], Some("obs_nvenc_h264_tex"));
        assert!(m.failed.is_empty());
    }

    fn video(rc: RateControl, q: QualityPreset) -> VideoConfig {
        VideoConfig {
            fps: 60,
            bitrate_kbps: 12000,
            output_height: None,
            encoder: "auto".into(),
            rate_control: rc,
            quality: q,
        }
    }

    fn get<'a>(s: &'a [(&'static str, SettingValue)], k: &str) -> Option<&'a SettingValue> {
        s.iter().find(|(n, _)| *n == k).map(|(_, v)| v)
    }

    #[test]
    fn encoder_settings_by_family_and_mode() {
        use SettingValue::*;
        let q = video(RateControl::Quality, QualityPreset::Medium);
        let n = encoder_settings("obs_nvenc_h264_tex", &q);
        assert_eq!(get(&n, "rate_control"), Some(&Str("CQP")));
        assert_eq!(get(&n, "cqp"), Some(&Int(24)));
        assert_eq!(get(&n, "preset"), Some(&Str("p4")));
        assert_eq!(get(&n, "lookahead"), Some(&Bool(false)));
        assert_eq!(get(&n, "adaptive_quantization"), Some(&Bool(false)));
        assert_eq!(get(&n, "multipass"), Some(&Str("disabled")));
        assert_eq!(get(&n, "keyint_sec"), Some(&Int(1)));
        assert_eq!(get(&n, "bitrate"), None);
        let x = encoder_settings("obs_x264", &video(RateControl::Quality, QualityPreset::High));
        assert_eq!(get(&x, "rate_control"), Some(&Str("CRF")));
        assert_eq!(get(&x, "crf"), Some(&Int(20)));
        assert_eq!(get(&x, "preset"), Some(&Str("veryfast")));
        let qs = encoder_settings("obs_qsv11_v2", &video(RateControl::Quality, QualityPreset::Low));
        assert_eq!(get(&qs, "target_usage"), Some(&Str("TU4")));
        assert_eq!(get(&qs, "qpb"), Some(&Int(28)));
        let a = encoder_settings("h264_texture_amf", &video(RateControl::Quality, QualityPreset::Low));
        assert_eq!(get(&a, "preset"), Some(&Str("speed")));
        // CBR keeps the bitrate.
        let c = encoder_settings("obs_nvenc_h264_tex", &video(RateControl::Cbr, QualityPreset::Medium));
        assert_eq!(get(&c, "rate_control"), Some(&Str("CBR")));
        assert_eq!(get(&c, "bitrate"), Some(&Int(12000)));
        assert_eq!(get(&c, "cqp"), None);
    }

    #[test]
    fn fallback_messages() {
        let all = s(&["obs_x264", "obs_nvenc_h264_tex"]);
        let p = plan_encoders("auto", &all, &[], (1920, 1080), None, 60);
        assert_eq!(fallback_message(&p[0], &p[0], &[]), None);
        let m = fallback_message(&p[0], &p[1], &s(&["obs_nvenc_h264_tex"])).unwrap();
        assert!(m.contains("NVENC did not start") && m.contains("720p60"), "{m}");
        let only = plan_encoders("auto", &s(&["obs_x264"]), &[], (1920, 1080), None, 60);
        let m = fallback_message(&only[0], &only[0], &[]).unwrap();
        assert!(m.contains("no GPU encoder is available"), "{m}");
    }

    fn w(exe: &str, pid: u32, game: bool, title: bool) -> WindowCandidate {
        WindowCandidate {
            full_exe: exe.into(),
            pid,
            is_game: game,
            has_title: title,
        }
    }

    #[test]
    fn window_matching() {
        let c = vec![
            w(r"C:\Windows\explorer.exe", 1, false, true),
            w(
                r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe",
                50,
                false,
                false,
            ),
            w(
                r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe",
                50,
                true,
                true,
            ),
            w(r"D:\Games\cs2.exe", 70, true, true),
        ];
        assert_eq!(pick_window(&c, Some("valorant-win64-shipping.exe"), None), Some(2));
        assert_eq!(pick_window(&c, Some(r"C:\x\cs2.exe"), None), Some(3));
        assert_eq!(pick_window(&c, None, Some(70)), Some(3));
        // pid beats name
        assert_eq!(pick_window(&c, Some("explorer.exe"), Some(70)), Some(3));
        assert_eq!(pick_window(&c, Some("notepad.exe"), None), None);
        assert_eq!(pick_window(&c, None, None), None);
        // ties go to the first window
        let t = vec![w("a.exe", 1, true, true), w("a.exe", 2, true, true)];
        assert_eq!(pick_window(&t, Some("A.EXE"), None), Some(0));
    }

    #[test]
    fn output_sizes() {
        assert_eq!(output_size(2560, 1440, None), (2560, 1440));
        assert_eq!(output_size(2560, 1440, Some(1080)), (1920, 1080));
        assert_eq!(output_size(1920, 1080, Some(1440)), (1920, 1080), "never upscale");
        assert_eq!(output_size(3440, 1440, Some(720)), (1720, 720));
        assert_eq!(output_size(1366, 768, Some(721)), (1282, 720));
    }
}
