//! Pure decision helpers: which encoder to use and which window to capture.

/// OBS encoder ids, most preferred first, for each family.
const NVENC: &[&str] = &["obs_nvenc_h264_tex", "jim_nvenc", "obs_nvenc_h264_soft", "ffmpeg_nvenc"];
const AMF: &[&str] = &["h264_texture_amf"];
const QSV: &[&str] = &["obs_qsv11_v2", "obs_qsv11", "obs_qsv11_soft_v2", "obs_qsv11_soft"];
pub const X264: &str = "obs_x264";

/// Pick an encoder id for the requested family ("auto" | "nvenc" | "amf" | "qsv" | "x264")
/// from the encoders OBS reports as available. Unknown or unavailable requests fall back
/// to "auto", which prefers hardware (NVENC, AMF, QSV) and ends with x264.
pub fn choose_encoder(requested: &str, available: &[String]) -> String {
    let has = |id: &str| available.iter().any(|a| a == id);
    let first = |list: &[&str]| list.iter().find(|id| has(id)).map(|s| s.to_string());
    let family = match requested.to_ascii_lowercase().as_str() {
        "nvenc" => first(NVENC),
        "amf" => first(AMF),
        "qsv" => first(QSV),
        "x264" => Some(X264.to_string()),
        _ => None,
    };
    family
        .or_else(|| first(NVENC))
        .or_else(|| first(AMF))
        .or_else(|| first(QSV))
        .unwrap_or_else(|| X264.to_string())
}

/// True for encoder ids that run on the GPU (used to decide whether to retry with x264).
pub fn is_hardware(id: &str) -> bool {
    id != X264
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
        assert_eq!(choose_encoder("auto", &s(&["obs_x264", "obs_qsv11_v2"])), "obs_qsv11_v2");
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
