//! Image handling: format sniffing, bounded decoding, thumbnails and downscaled JPEG copies.
//!
//! Every input here is untrusted (uploads, CDN downloads), so decoding runs with strict size
//! limits (dimensions and pixel count read from the header first, then a decoder allocation
//! cap), and callers run these functions through [`run`]: on the blocking pool, at most
//! [`DECODE_SLOTS`] at a time, which bounds the memory all decodes together can take.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ExtendedColorType, ImageFormat, ImageReader, Limits};

/// Largest accepted width or height, in pixels.
pub const MAX_DIMENSION: u32 = 16_384;
/// Largest accepted image, in pixels (100 MP: beyond any model output or camera upload).
pub const MAX_PIXELS: u64 = 100_000_000;
/// Largest decoder allocation.
const MAX_DECODE_ALLOC: u64 = 512 << 20;
/// Image jobs running at once (each may take up to about `MAX_DECODE_ALLOC`).
pub const DECODE_SLOTS: usize = 2;

/// Run image work `f` on the blocking pool, at most [`DECODE_SLOTS`] at a time.
pub async fn run<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    let slots = SLOTS.get_or_init(|| tokio::sync::Semaphore::new(DECODE_SLOTS));
    let _permit = slots.acquire().await.map_err(|e| format!("image queue: {e}"))?;
    tokio::task::spawn_blocking(f).await.map_err(|e| format!("image task: {e}"))
}
/// Thumbnails fit in this square.
pub const THUMB_EDGE: u32 = 512;
const JPEG_QUALITY: u8 = 85;

/// The image formats Photo Studio stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
        }
    }
    pub fn mime(self) -> &'static str {
        match self {
            Format::Png => "image/png",
            Format::Jpeg => "image/jpeg",
            Format::Webp => "image/webp",
        }
    }
    pub fn from_ext(ext: &str) -> Option<Format> {
        match ext {
            "png" => Some(Format::Png),
            "jpg" | "jpeg" => Some(Format::Jpeg),
            "webp" => Some(Format::Webp),
            _ => None,
        }
    }
    fn image_format(self) -> ImageFormat {
        match self {
            Format::Png => ImageFormat::Png,
            Format::Jpeg => ImageFormat::Jpeg,
            Format::Webp => ImageFormat::WebP,
        }
    }
}

/// The format of `bytes` from its magic number (the declared content type is not trusted).
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(Format::Jpeg)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(Format::Webp)
    } else {
        None
    }
}

/// Decode `bytes` as `format` within the size limits.
pub fn decode(bytes: &[u8], format: Format) -> Result<DynamicImage, String> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    let (w, h) = ImageReader::with_format(Cursor::new(bytes), format.image_format())
        .into_dimensions()
        .map_err(|e| format!("can't read the {} image: {e}", format.ext()))?;
    if w > MAX_DIMENSION || h > MAX_DIMENSION || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(format!("the image is {w}×{h}: at most {MAX_DIMENSION} px a side and {} megapixels", MAX_PIXELS / 1_000_000));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format.image_format());
    reader.limits(limits);
    let img = reader.decode().map_err(|e| format!("can't decode the {} image: {e}", format.ext()))?;
    if img.width() == 0 || img.height() == 0 {
        return Err("the image is empty".into());
    }
    Ok(img)
}

/// `img` scaled down (never up) to fit in `max_edge` × `max_edge`, encoded as JPEG.
pub fn jpeg_fit(img: &DynamicImage, max_edge: u32, fast: bool) -> Result<Vec<u8>, String> {
    let scaled;
    let src = if img.width() > max_edge || img.height() > max_edge {
        scaled = if fast {
            img.thumbnail(max_edge, max_edge)
        } else if img.width() > max_edge.saturating_mul(2) || img.height() > max_edge.saturating_mul(2) {
            // Integer sampling down to twice the size first: the filtered pass then works on a
            // small image (a float intermediate of a 16k-wide source would take ~300 MB).
            let edge = max_edge.saturating_mul(2);
            img.thumbnail(edge, edge).resize(max_edge, max_edge, FilterType::CatmullRom)
        } else {
            img.resize(max_edge, max_edge, FilterType::CatmullRom)
        };
        &scaled
    } else {
        img
    };
    let rgb = src.to_rgb8();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode(rgb.as_raw(), rgb.width(), rgb.height(), ExtendedColorType::Rgb8)
        .map_err(|e| format!("can't encode JPEG: {e}"))?;
    Ok(out)
}

/// What storing an image needs: its format, size and a thumbnail.
pub struct Processed {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub thumb: Vec<u8>,
}

/// Sniff, decode and thumbnail `bytes`.
pub fn process(bytes: &[u8]) -> Result<Processed, String> {
    let format = sniff(bytes).ok_or("not a PNG, JPEG or WebP image")?;
    let img = decode(bytes, format)?;
    let thumb = jpeg_fit(&img, THUMB_EDGE, true)?;
    Ok(Processed { format, width: img.width(), height: img.height(), thumb })
}

/// `bytes` (any stored format) as a JPEG that fits in `max_edge`, for Claude.
pub fn jpeg_for_claude(bytes: &[u8], max_edge: u32) -> Result<Vec<u8>, String> {
    let format = sniff(bytes).ok_or("not a PNG, JPEG or WebP image")?;
    let img = decode(bytes, format)?;
    jpeg_fit(&img, max_edge, false)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A test PNG of the given size.
    pub fn png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(width, height, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(img).write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    pub fn dims(jpeg_or_png: &[u8]) -> (u32, u32) {
        let f = sniff(jpeg_or_png).unwrap();
        let img = decode(jpeg_or_png, f).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(&png(2, 2)), Some(Format::Png));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(Format::Jpeg));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::Webp));
        assert_eq!(sniff(b"GIF89a"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn thumbnails_and_downscales() {
        let p = process(&png(1200, 600)).unwrap();
        assert_eq!((p.width, p.height), (1200, 600));
        assert_eq!(dims(&p.thumb), (512, 256));
        let small = process(&png(100, 50)).unwrap();
        assert_eq!(dims(&small.thumb), (100, 50), "never upscaled");
        let j = jpeg_for_claude(&png(2000, 1000), 1568).unwrap();
        assert_eq!(sniff(&j), Some(Format::Jpeg));
        assert_eq!(dims(&j), (1568, 784));
    }

    #[test]
    fn oversized_images_are_refused_before_decoding() {
        // A flat 12000×9000 image (108 MP) compresses to a small PNG; it must be refused from its
        // header, without allocating the pixels.
        let img = image::GrayImage::new(12_000, 9_000);
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(img).write_to(&mut out, ImageFormat::Png).unwrap();
        let bytes = out.into_inner();
        assert!(bytes.len() < 1 << 20);
        let e = process(&bytes).err().unwrap();
        assert!(e.contains("megapixels"), "{e}");
        let big = jpeg_for_claude(&png(5000, 2500), 1024).unwrap();
        assert_eq!(dims(&big), (1024, 512), "two-pass downscale keeps the size exact");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn decodes_are_bounded() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = vec![];
        for _ in 0..8 {
            let (running, peak) = (running.clone(), peak.clone());
            tasks.push(tokio::spawn(run(move || {
                let n = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(30));
                running.fetch_sub(1, Ordering::SeqCst);
            })));
        }
        for t in tasks {
            t.await.unwrap().unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= DECODE_SLOTS);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(process(b"not an image").is_err());
        let mut truncated = png(64, 64);
        truncated.truncate(40);
        assert!(process(&truncated).is_err());
    }
}
