//! Thin helpers around the bundled ffmpeg/ffprobe executables.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use anyhow::{bail, Context, Result};
use image::GrayImage;

/// Paths to ffmpeg and ffprobe.
#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Set while a game is running: video decoding then stays off the GPU and uses few threads, so
/// analysing a finished recording never competes with the game for frames.
static GAME_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tell the decoders whether a game is running right now.
pub fn set_game_running(on: bool) {
    GAME_RUNNING.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn game_running() -> bool {
    GAME_RUNNING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Decoder options for analysis: GPU decoding when nothing else needs the GPU; with a game
/// running, software decoding on two threads.
fn decode_args() -> &'static [&'static str] {
    if game_running() {
        &["-threads", "2"]
    } else {
        &["-hwaccel", "auto"]
    }
}

/// Decoder options when only keyframes are wanted: the decoder skips every other frame, which
/// is far cheaper than decoding the whole stream (software decoding, since GPU decoders ignore
/// the skip).
fn keyframe_args() -> &'static [&'static str] {
    if game_running() {
        &["-skip_frame", "nokey", "-threads", "2"]
    } else {
        &["-skip_frame", "nokey"]
    }
}

#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Every ffmpeg Recap starts is background work: let the game (and everything else) go first.
    const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
    cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
}
#[cfg(not(windows))]
fn no_window(_cmd: &mut Command) {}

/// The part of a frame that holds the game picture, excluding black letterbox/pillarbox bars
/// (e.g. a 16:9 game recorded on a 21:9 monitor). Detector regions are fractions of this area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl ContentRect {
    pub fn filter(&self) -> String {
        format!("crop={}:{}:{}:{}", self.w, self.h, self.x, self.y)
    }
}

/// Parse the last `crop=w:h:x:y` that ffmpeg's cropdetect printed; ignore it when the bars are
/// negligible or the result is implausible.
pub fn parse_cropdetect(stderr: &str, width: u32, height: u32) -> Option<ContentRect> {
    let last = stderr.rsplit("crop=").next().filter(|_| stderr.contains("crop="))?;
    let nums: Vec<u32> = last
        .split(|c: char| !c.is_ascii_digit())
        .filter(|t| !t.is_empty())
        .take(4)
        .filter_map(|t| t.parse().ok())
        .collect();
    let [w, h, x, y] = nums[..] else { return None };
    let r = ContentRect { x, y, w, h };
    let bars = (width.saturating_sub(w)) + (height.saturating_sub(h));
    let plausible = w >= width / 2 && h >= height / 2 && x + w <= width && y + h <= height;
    // Only treat it as letterboxing if the bars are a real share of the frame (>= 2%).
    (plausible && bars * 50 >= width.min(height)).then_some(r)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    /// Average frames per second (0 when unknown).
    pub fps: f32,
}

/// Parse an ffprobe rate such as `60/1` or `30000/1001`.
fn parse_rate(r: &str) -> f32 {
    match r.split_once('/') {
        Some((n, d)) => match (n.parse::<f32>(), d.parse::<f32>()) {
            (Ok(n), Ok(d)) if d > 0.0 => n / d,
            _ => 0.0,
        },
        None => r.parse().unwrap_or(0.0),
    }
}

impl Ffmpeg {
    /// Look for ffmpeg next to `dir` (bundled), then fall back to PATH.
    pub fn locate(dirs: &[PathBuf]) -> Self {
        let exe = |name: &str| {
            let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
            dirs.iter()
                .map(|d| d.join(&file))
                .find(|p| p.exists())
                .unwrap_or_else(|| PathBuf::from(file))
        };
        Self {
            ffmpeg: exe("ffmpeg"),
            ffprobe: exe("ffprobe"),
        }
    }

    pub fn command(&self) -> Command {
        let mut c = Command::new(&self.ffmpeg);
        no_window(&mut c);
        c.arg("-hide_banner").arg("-loglevel").arg("error");
        c
    }

    fn probe_command(&self) -> Command {
        let mut c = Command::new(&self.ffprobe);
        no_window(&mut c);
        c
    }

    /// An ffprobe command (no console window on Windows).
    pub fn ffprobe_command(&self) -> Command {
        self.probe_command()
    }

    pub fn probe(&self, video: &Path) -> Result<VideoInfo> {
        let out = self
            .probe_command()
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,avg_frame_rate,r_frame_rate:format=duration",
                "-of",
                "json",
            ])
            .arg(video)
            .output()
            .with_context(|| format!("running {}", self.ffprobe.display()))?;
        if !out.status.success() {
            bail!("ffprobe failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
        let s = &v["streams"][0];
        let width = s["width"].as_u64().context("no video width")? as u32;
        let height = s["height"].as_u64().context("no video height")? as u32;
        let duration = v["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0);
        let rate = |k: &str| s[k].as_str().map(parse_rate).filter(|f| f.is_finite() && *f > 0.0 && *f < 1000.0);
        Ok(VideoInfo {
            width,
            height,
            duration_ms: (duration * 1000.0) as u64,
            fps: rate("avg_frame_rate").or_else(|| rate("r_frame_rate")).unwrap_or(0.0),
        })
    }

    /// Typical spacing of keyframes (ms), from the packet flags of the first minute (nothing is
    /// decoded). None when it cannot be told.
    pub fn keyframe_interval_ms(&self, video: &Path) -> Option<u64> {
        let out = self
            .probe_command()
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-read_intervals",
                "%+60",
                "-show_entries",
                "packet=pts_time,flags",
                "-of",
                "csv=p=0",
            ])
            .arg(video)
            .output()
            .ok()?;
        keyframe_interval(&String::from_utf8_lossy(&out.stdout))
    }

    /// True if the file has at least one audio stream.
    pub fn has_audio(&self, video: &Path) -> bool {
        self.probe_command()
            .args([
                "-v",
                "error",
                "-select_streams",
                "a",
                "-show_entries",
                "stream=index",
                "-of",
                "csv=p=0",
            ])
            .arg(video)
            .output()
            .map(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty())
            .unwrap_or(false)
    }

    /// Find black bars around the game picture by sampling ~12 s from a third into the video.
    pub fn content_rect(&self, video: &Path, info: &VideoInfo) -> Option<ContentRect> {
        let start = info.duration_ms / 3;
        let out = self
            .command()
            .args(["-loglevel", "info", "-ss"])
            .arg(format_secs(start))
            .arg("-i")
            .arg(video)
            .args([
                "-t",
                "12",
                "-an",
                "-vf",
                "fps=4,cropdetect=limit=24:round=2:reset=0",
                "-f",
                "null",
                "-",
            ])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        parse_cropdetect(&String::from_utf8_lossy(&out.stderr), info.width, info.height)
    }

    /// Extract one frame as PNG bytes at full resolution (optionally only the game picture).
    pub fn frame_png(&self, video: &Path, t_ms: u64, crop: Option<ContentRect>) -> Result<Vec<u8>> {
        let mut cmd = self.command();
        cmd.arg("-ss").arg(format_secs(t_ms)).arg("-i").arg(video).args(["-frames:v", "1"]);
        if let Some(c) = crop {
            cmd.arg("-vf").arg(c.filter());
        }
        let out = cmd
            .args(["-f", "image2pipe", "-vcodec", "png", "-"])
            .stdin(Stdio::null())
            .output()
            .context("running ffmpeg")?;
        if !out.status.success() || out.stdout.is_empty() {
            bail!("could not extract frame: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(out.stdout)
    }

    /// Write a small JPEG thumbnail.
    pub fn thumbnail(&self, video: &Path, t_ms: u64, out_path: &Path, width: u32) -> Result<()> {
        let status = self
            .command()
            .arg("-y")
            .arg("-ss")
            .arg(format_secs(t_ms))
            .arg("-i")
            .arg(video)
            .args(["-frames:v", "1", "-vf"])
            .arg(format!("scale={width}:-2"))
            .args(["-q:v", "4"])
            .arg(out_path)
            .stdin(Stdio::null())
            .status()?;
        if !status.success() {
            bail!("thumbnail failed for {}", video.display());
        }
        Ok(())
    }

    /// Decode a video as grayscale frames of height `height` sampled at `fps`, cropped to the game
    /// picture when `crop` is given.
    pub fn gray_frames(&self, video: &Path, fps: f32, info: &VideoInfo, height: u32, crop: Option<ContentRect>) -> Result<GrayFrames> {
        self.analysis_frames(video, fps, info, height, crop, false)
    }

    /// Like [`Ffmpeg::gray_frames`]; with `color` the frames are RGB (3 bytes per pixel, for
    /// detectors that filter by colour). Use [`GrayFrames::is_color`] to tell them apart.
    pub fn analysis_frames(
        &self,
        video: &Path,
        fps: f32,
        info: &VideoInfo,
        height: u32,
        crop: Option<ContentRect>,
        color: bool,
    ) -> Result<GrayFrames> {
        self.analysis_frames_opts(video, fps, info, height, crop, color, Decode::default())
    }

    /// [`Ffmpeg::analysis_frames`] over part of the video and/or from keyframes only.
    #[allow(clippy::too_many_arguments)]
    pub fn analysis_frames_opts(
        &self,
        video: &Path,
        fps: f32,
        info: &VideoInfo,
        height: u32,
        crop: Option<ContentRect>,
        color: bool,
        opts: Decode,
    ) -> Result<GrayFrames> {
        let (src_w, src_h) = crop.map(|c| (c.w, c.h)).unwrap_or((info.width, info.height));
        let height = height.min(src_h).max(2) & !1;
        let width = (((src_w as f64 * height as f64 / src_h as f64).round() as u32).max(2)) & !1;
        let crop_filter = crop.map(|c| format!("{},", c.filter())).unwrap_or_default();
        let mut cmd = self.command();
        cmd.args(if opts.keyframes_only { keyframe_args() } else { decode_args() });
        if let Some((start, dur)) = opts.range {
            cmd.args(["-ss", &format_secs(start), "-t", &format_secs(dur.max(1))]);
        }
        let mut child = cmd
            .arg("-i")
            .arg(video)
            .args(["-an", "-sn", "-vf"])
            .arg(if opts.keyframes_only {
                // Keyframes come at uneven times (encoders add them at scene cuts): scale each
                // once, then let `fps` repeat them on a regular clock. Callers skip repeats.
                format!(
                    "{crop_filter}scale={width}:{height}:flags=area,format={},fps={fps}",
                    if color { "rgb24" } else { "gray" }
                )
            } else {
                format!(
                    "fps={fps},{crop_filter}scale={width}:{height}:flags=area,format={}",
                    if color { "rgb24" } else { "gray" }
                )
            })
            .args(["-f", "rawvideo", "-pix_fmt", if color { "rgb24" } else { "gray" }, "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting ffmpeg")?;
        let stdout = child.stdout.take().context("no ffmpeg stdout")?;
        Ok(GrayFrames {
            child,
            stdout,
            start_ms: opts.range.map(|r| r.0).unwrap_or(0),
            width,
            height,
            fps,
            index: 0,
            color,
        })
    }
}

impl Ffmpeg {
    /// Decode `[start_ms, start_ms + dur_ms)` as full-resolution RGB frames at `fps`, cropped to
    /// the game picture when `crop` is given. Timestamps are from the start of the video.
    #[allow(clippy::too_many_arguments)]
    pub fn rgb_frames(
        &self,
        video: &Path,
        start_ms: u64,
        dur_ms: u64,
        fps: f32,
        info: &VideoInfo,
        crop: Option<ContentRect>,
        keyframes_only: bool,
    ) -> Result<RgbFrames> {
        let (width, height) = crop.map(|c| (c.w, c.h)).unwrap_or((info.width, info.height));
        let crop_filter = crop.map(|c| format!("{},", c.filter())).unwrap_or_default();
        let mut child = self
            .command()
            .args(if keyframes_only { keyframe_args() } else { decode_args() })
            .args(["-ss", &format_secs(start_ms), "-t", &format_secs(dur_ms.max(1)), "-i"])
            .arg(video)
            .args(["-an", "-sn", "-vf"])
            .arg(format!("fps={fps},{crop_filter}format=rgb24"))
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting ffmpeg")?;
        let stdout = child.stdout.take().context("no ffmpeg stdout")?;
        Ok(RgbFrames {
            child,
            stdout,
            width,
            height,
            fps,
            start_ms,
            index: 0,
        })
    }
}

/// Iterator over decoded RGB frames with their timestamps.
pub struct RgbFrames {
    child: Child,
    stdout: std::process::ChildStdout,
    pub width: u32,
    pub height: u32,
    fps: f32,
    start_ms: u64,
    index: u64,
}

impl Iterator for RgbFrames {
    type Item = (u64, image::RgbImage);
    fn next(&mut self) -> Option<Self::Item> {
        let mut buf = vec![0u8; (self.width * self.height * 3) as usize];
        if self.stdout.read_exact(&mut buf).is_err() {
            return None;
        }
        let t_ms = self.start_ms + ((self.index as f64) * 1000.0 / self.fps as f64).round() as u64;
        self.index += 1;
        image::RgbImage::from_raw(self.width, self.height, buf).map(|img| (t_ms, img))
    }
}

impl Drop for RgbFrames {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Iterator over decoded grayscale frames with their timestamps.
pub struct GrayFrames {
    child: Child,
    stdout: std::process::ChildStdout,
    /// Where the decoded range starts in the video.
    start_ms: u64,
    pub width: u32,
    pub height: u32,
    fps: f32,
    index: u64,
    color: bool,
}

/// A frame from [`Ffmpeg::analysis_frames`].
pub enum AnalysisFrame {
    Gray(GrayImage),
    Rgb(image::RgbImage),
}

impl GrayFrames {
    pub fn is_color(&self) -> bool {
        self.color
    }

    /// The next frame, gray or colour depending on how the stream was opened.
    pub fn next_frame(&mut self) -> Option<(u64, AnalysisFrame)> {
        let bpp = if self.color { 3 } else { 1 };
        let mut buf = vec![0u8; (self.width * self.height * bpp) as usize];
        if self.stdout.read_exact(&mut buf).is_err() {
            return None;
        }
        let t_ms = self.start_ms + ((self.index as f64) * 1000.0 / self.fps as f64).round() as u64;
        self.index += 1;
        let frame = if self.color {
            AnalysisFrame::Rgb(image::RgbImage::from_raw(self.width, self.height, buf)?)
        } else {
            AnalysisFrame::Gray(GrayImage::from_raw(self.width, self.height, buf)?)
        };
        Some((t_ms, frame))
    }
}

impl Iterator for GrayFrames {
    type Item = (u64, GrayImage);
    fn next(&mut self) -> Option<Self::Item> {
        match self.next_frame()? {
            (t, AnalysisFrame::Gray(g)) => Some((t, g)),
            (t, AnalysisFrame::Rgb(c)) => Some((t, image::imageops::grayscale(&c))),
        }
    }
}

impl Drop for GrayFrames {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// How much of a video to decode for analysis.
#[derive(Debug, Clone, Copy, Default)]
pub struct Decode {
    /// `(start_ms, dur_ms)`; the whole video when None.
    pub range: Option<(u64, u64)>,
    /// Decode keyframes only (frames in between repeat the last keyframe).
    pub keyframes_only: bool,
}

/// Median spacing of keyframes in ffprobe `packet=pts_time,flags` CSV output.
pub fn keyframe_interval(csv: &str) -> Option<u64> {
    let times: Vec<f64> = csv
        .lines()
        .filter_map(|l| {
            let (t, flags) = l.split_once(',')?;
            flags.starts_with('K').then(|| t.trim().parse::<f64>().ok()).flatten()
        })
        .collect();
    let mut gaps: Vec<u64> = times
        .windows(2)
        .map(|w| ((w[1] - w[0]) * 1000.0).round().max(0.0) as u64)
        .filter(|g| *g > 0)
        .collect();
    if gaps.len() < 2 {
        return None;
    }
    gaps.sort_unstable();
    Some(gaps[gaps.len() / 2])
}

pub fn format_secs(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframe_spacing() {
        let csv = "0.000000,K__\n0.016667,___\n1.000000,K__\n1.016667,___\n2.000000,K_\n3.000000,K__\n";
        assert_eq!(keyframe_interval(csv), Some(1000));
        assert_eq!(keyframe_interval("0.0,K__\n"), None);
    }

    #[test]
    fn parses_frame_rates() {
        assert_eq!(parse_rate("60/1"), 60.0);
        assert!((parse_rate("30000/1001") - 29.97).abs() < 0.01);
        assert_eq!(parse_rate("0/0"), 0.0);
        assert_eq!(parse_rate("25"), 25.0);
    }

    #[test]
    fn cropdetect_parsing() {
        let log = "[Parsed_cropdetect_1 @ 0x1] x1:0 ... crop=1920:1080:0:0\n[Parsed_cropdetect_1 @ 0x1] ... crop=1920:1080:336:0\n";
        assert_eq!(
            parse_cropdetect(log, 2592, 1080),
            Some(ContentRect {
                x: 336,
                y: 0,
                w: 1920,
                h: 1080
            })
        );
        // No bars worth cropping.
        assert_eq!(parse_cropdetect("crop=1920:1076:0:2", 1920, 1080), None);
        // Implausible (mostly black scene).
        assert_eq!(parse_cropdetect("crop=400:300:10:10", 1920, 1080), None);
        assert_eq!(parse_cropdetect("no crop here", 1920, 1080), None);
    }

    #[test]
    fn formats_seconds() {
        assert_eq!(format_secs(0), "0.000");
        assert_eq!(format_secs(61_005), "61.005");
    }
}
