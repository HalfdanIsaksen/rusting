//! Reading PNG/JPEG files for File → Import (ADR-015).

use document::drawing::ImageData;
use std::io::Cursor;
use std::path::Path;

/// File extensions the import dialog offers.
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

/// Reads `path` and checks that it is a PNG or JPEG whose size can be read.
/// The file's bytes are kept as they are; the renderer decodes them.
pub fn read_image(path: &Path) -> Result<ImageData, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    image_data(bytes)
}

/// Wraps an encoded PNG/JPEG, reading its size from the header and checking
/// that the whole picture decodes.
fn image_data(bytes: Vec<u8>) -> Result<ImageData, String> {
    let reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    match reader.format() {
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg) => {}
        _ => return Err("Only PNG and JPEG pictures can be imported.".into()),
    }
    let picture = reader.decode().map_err(|e| e.to_string())?;
    let (width, height) = (picture.width(), picture.height());
    if width == 0 || height == 0 {
        return Err("The picture has no pixels.".into());
    }
    Ok(ImageData::new(width, height, bytes))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(3, 2, image::Rgb([10, 20, 30]))
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn reads_png_and_jpeg_sizes_and_keeps_the_bytes() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let bytes = encoded(format);
            let data = image_data(bytes.clone()).unwrap();
            assert_eq!((data.width, data.height), (3, 2));
            assert_eq!(&*data.bytes, bytes.as_slice());
        }
    }

    #[test]
    fn rejects_other_and_broken_files() {
        assert!(image_data(b"hello".to_vec()).is_err());
        let mut png = encoded(image::ImageFormat::Png);
        png.truncate(png.len() / 2);
        assert!(image_data(png).is_err());
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(read_image(Path::new("/no/such/picture.png")).is_err());
    }
}
