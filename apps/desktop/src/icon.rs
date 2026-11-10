//! The app icon: the website logo ([`ui::logo`]) on the window (Windows and
//! Linux) and in the Dock (macOS).
//!
//! The release ships a bare binary rather than an `.app` bundle, so on macOS
//! the Dock icon is set at runtime instead of from an `.icns` file.

use winit::window::Icon;

/// Side of the window icon, in pixels.
const WINDOW_ICON_SIZE: u32 = 256;

/// The icon for the window's title bar and taskbar entry. macOS ignores it.
pub fn window_icon() -> Option<Icon> {
    let pixels = ui::logo::rgba(WINDOW_ICON_SIZE, 0.0, ui::logo::ICON_COLORS);
    Icon::from_rgba(pixels, WINDOW_ICON_SIZE, WINDOW_ICON_SIZE)
        .inspect_err(|e| tracing::warn!("window icon: {e}"))
        .ok()
}

/// Sets the Dock icon. Must run on the main thread after the event loop
/// exists; does nothing anywhere else.
#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::AllocAnyThread;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    /// Dock icon side, in pixels (256 points on a Retina display).
    const SIZE: u32 = 512;
    /// Apple's icon grid leaves about 100/1024 of transparent margin per side.
    const INSET: f32 = 100.0 / 1024.0;

    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("dock icon: not on the main thread");
        return;
    };
    let pixels = ui::logo::rgba(SIZE, INSET, ui::logo::ICON_COLORS);
    let png = match encode_png(&pixels, SIZE) {
        Ok(png) => png,
        Err(e) => {
            tracing::warn!("dock icon: {e}");
            return;
        }
    };
    let data = NSData::with_bytes(&png);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        tracing::warn!("dock icon: AppKit could not read the image");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: on the main thread (checked above) with a valid image; the
    // method is only unsafe because AppKit may not accept `None`.
    #[allow(unsafe_code)]
    unsafe {
        app.setApplicationIconImage(Some(&image));
    }
}

/// No Dock outside macOS.
#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon() {}

/// `size` × `size` RGBA pixels as a PNG file.
#[cfg(target_os = "macos")]
fn encode_png(pixels: &[u8], size: u32) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)?;
    writer.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn builds_a_window_icon() {
        assert!(super::window_icon().is_some());
    }
}
