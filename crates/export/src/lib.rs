//! Encoding exported frames to files. No GPU or UI code.
//!
//! Frames arrive as [`RgbaImage`]s: 8-bit, straight-alpha, sRGB-encoded
//! pixels, top row first. This crate writes them as PNG or JPEG
//! ([`write_image`]), numbered image sequences ([`sequence_path`]), or H.264
//! MP4 video ([`Mp4Encoder`], built on the bundled OpenH264 encoder and a
//! pure-Rust MP4 muxer; see ADR-008), or looping animated GIFs
//! ([`GifEncoder`], up to [`MAX_GIF_SECONDS`] long). [`ExportWorker`] runs the encoding on a
//! background thread so the UI can keep rendering frames.

mod gif_file;
mod image_file;
mod mp4_file;
mod worker;
pub mod yuv;

pub use gif_file::{GifEncoder, GifSettings, MAX_GIF_SECONDS, check_gif_length};
pub use image_file::{ImageFormat, RgbaImage, flatten, sequence_path, write_image};
pub use mp4_file::{Mp4Encoder, VideoSettings};
pub use worker::{ExportTarget, ExportWorker, WorkerEvent};

/// Errors from writing an export.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// A file could not be created or written.
    #[error("could not write {path}: {source}")]
    Io {
        /// The file being written.
        path: std::path::PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// The PNG or JPEG encoder failed.
    #[error("could not encode image: {0}")]
    Image(#[from] image::ImageError),
    /// The H.264 encoder failed.
    #[error("video encoder failed: {0}")]
    Encoder(String),
    /// The MP4 muxer failed.
    #[error("could not write MP4: {0}")]
    Mux(#[from] mp4::Error),
    /// A frame did not match what the export was set up for.
    #[error("{0}")]
    InvalidFrame(String),
    /// The animation plays too long for the chosen format (GIF).
    #[error(
        "animation too long for GIF ({seconds:.1} s, at most {max_seconds:.0} s): export it as a video or image sequence instead"
    )]
    TooLong {
        /// How long the exported frames play.
        seconds: f64,
        /// The longest allowed.
        max_seconds: f64,
    },
    /// The export was cancelled.
    #[error("export cancelled")]
    Cancelled,
}
