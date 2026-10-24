//! Offline text recognition with the OCR engine built into Windows 10/11 (`Windows.Media.Ocr`).
//! Nothing is downloaded or sent anywhere. On other platforms [`recognize`] returns an error.

/// Recognise the text in an 8-bit grayscale image (`width * height` bytes). Lines are joined
/// with spaces.
pub fn recognize(gray: &[u8], width: u32, height: u32) -> anyhow::Result<String> {
    anyhow::ensure!(gray.len() == (width * height) as usize, "image buffer does not match its size");
    imp::recognize(gray, width, height)
}

/// Whether an OCR engine is available for the user's languages.
pub fn available() -> bool {
    imp::available()
}

#[cfg(windows)]
mod imp {
    use anyhow::{anyhow, Context, Result};
    use windows::{
        Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap},
        Media::Ocr::OcrEngine,
        Security::Cryptography::CryptographicBuffer,
    };

    thread_local! {
        static ENGINE: Option<OcrEngine> = OcrEngine::TryCreateFromUserProfileLanguages().ok();
    }

    pub fn available() -> bool {
        ENGINE.with(|e| e.is_some())
    }

    pub fn recognize(gray: &[u8], width: u32, height: u32) -> Result<String> {
        // The engine accepts BGRA; expand the grayscale bytes.
        let mut bgra = Vec::with_capacity(gray.len() * 4);
        for &v in gray {
            bgra.extend_from_slice(&[v, v, v, 255]);
        }
        let max = OcrEngine::MaxImageDimension().unwrap_or(2600);
        anyhow::ensure!(
            width <= max && height <= max,
            "image too large for OCR ({width}x{height}, max {max})"
        );
        let buffer = CryptographicBuffer::CreateFromByteArray(&bgra).context("creating OCR buffer")?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, width as i32, height as i32)
            .context("creating OCR bitmap")?;
        ENGINE.with(|engine| {
            let engine = engine.as_ref().ok_or_else(|| anyhow!("no Windows OCR language is installed"))?;
            let result = engine
                .RecognizeAsync(&bitmap)
                .context("starting OCR")?
                .join()
                .context("running OCR")?;
            let mut lines = Vec::new();
            for line in result.Lines().context("reading OCR lines")? {
                lines.push(line.Text().context("reading OCR text")?.to_string());
            }
            Ok(lines.join(" "))
        })
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn available() -> bool {
        false
    }

    pub fn recognize(_gray: &[u8], _width: u32, _height: u32) -> anyhow::Result<String> {
        anyhow::bail!("text recognition uses Windows OCR and is only available on Windows")
    }
}
