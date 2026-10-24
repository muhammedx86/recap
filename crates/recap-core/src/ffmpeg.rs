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

#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
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

    pub fn probe(&self, video: &Path) -> Result<VideoInfo> {
        let out = self
            .probe_command()
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height:format=duration",
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
        Ok(VideoInfo {
            width,
            height,
            duration_ms: (duration * 1000.0) as u64,
        })
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
        let (src_w, src_h) = crop.map(|c| (c.w, c.h)).unwrap_or((info.width, info.height));
        let height = height.min(src_h).max(2) & !1;
        let width = (((src_w as f64 * height as f64 / src_h as f64).round() as u32).max(2)) & !1;
        let crop_filter = crop.map(|c| format!("{},", c.filter())).unwrap_or_default();
        let mut child = self
            .command()
            .args(["-hwaccel", "auto", "-i"])
            .arg(video)
            .args(["-an", "-sn", "-vf"])
            .arg(format!("fps={fps},{crop_filter}scale={width}:{height}:flags=area,format=gray"))
            .args(["-f", "rawvideo", "-pix_fmt", "gray", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("starting ffmpeg")?;
        let stdout = child.stdout.take().context("no ffmpeg stdout")?;
        Ok(GrayFrames {
            child,
            stdout,
            width,
            height,
            fps,
            index: 0,
        })
    }
}

/// Iterator over decoded grayscale frames with their timestamps.
pub struct GrayFrames {
    child: Child,
    stdout: std::process::ChildStdout,
    pub width: u32,
    pub height: u32,
    fps: f32,
    index: u64,
}

impl Iterator for GrayFrames {
    type Item = (u64, GrayImage);
    fn next(&mut self) -> Option<Self::Item> {
        let mut buf = vec![0u8; (self.width * self.height) as usize];
        if self.stdout.read_exact(&mut buf).is_err() {
            return None;
        }
        let t_ms = ((self.index as f64) * 1000.0 / self.fps as f64).round() as u64;
        self.index += 1;
        GrayImage::from_raw(self.width, self.height, buf).map(|img| (t_ms, img))
    }
}

impl Drop for GrayFrames {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn format_secs(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

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
