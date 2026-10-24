//! Text recognition for screen readers: the OCR engine built into Windows (offline).

use std::sync::Arc;

use image::GrayImage;
use recap_core::readers::TextReader;

struct WindowsOcr;

impl TextReader for WindowsOcr {
    fn read_text(&self, img: &GrayImage) -> anyhow::Result<String> {
        recap_ocr::recognize(img.as_raw(), img.width(), img.height())
    }
}

/// The OCR engine, or None when it is unavailable (not Windows, or no OCR language installed).
pub fn text_reader() -> Option<Arc<dyn TextReader>> {
    if recap_ocr::available() {
        Some(Arc::new(WindowsOcr))
    } else {
        log::warn!("text recognition unavailable: text readers are skipped (choice readers still run)");
        None
    }
}
