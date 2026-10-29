//! Encoding exported frames to files. No GPU or UI code.
//!
//! Frames arrive as [`RgbaImage`]s: 8-bit, straight-alpha, sRGB-encoded
//! pixels, top row first. This crate writes them as PNG or JPEG
//! ([`write_image`]), numbered image sequences ([`sequence_path`]), or H.264
//! MP4 video ([`Mp4Encoder`], built on the bundled OpenH264 encoder and a
//! pure-Rust MP4 muxer; see ADR-008). [`ExportWorker`] runs the encoding on a
//! background thread so the UI can keep rendering frames.

mod image_file;
mod mp4_file;
mod worker;
pub mod yuv;

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
    /// The export was cancelled.
    #[error("export cancelled")]
    Cancelled,
}
