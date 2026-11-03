//! H.264 MP4 or MOV video: OpenH264 (bundled, built from source) encodes,
//! the pure-Rust `mp4` crate muxes. A `.mov` path gets QuickTime branding;
//! the boxes are the same. See ADR-008.

use crate::ExportError;
use crate::RgbaImage;
use crate::yuv::{even, pad_to_even, rgba_to_yuv420};
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode, VuiConfig,
};
use openh264::formats::YUVBuffer;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// NAL unit types the muxer handles specially.
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;
/// NAL types that don't belong in samples: parameter sets (stored in the
/// `avcC` box) and access unit delimiters.
const NAL_AUD: u8 = 9;

/// What the video looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoSettings {
    /// Frame width in pixels. Odd sizes are padded up by one pixel.
    pub width: u32,
    /// Frame height in pixels. Odd sizes are padded up by one pixel.
    pub height: u32,
    /// Frame rate numerator (e.g. 24000).
    pub fps_numerator: u32,
    /// Frame rate denominator (e.g. 1001).
    pub fps_denominator: u32,
}

impl VideoSettings {
    /// The encoded frame size: both sides even.
    pub fn encoded_size(&self) -> (u32, u32) {
        (even(self.width), even(self.height))
    }

    fn fps(&self) -> f64 {
        f64::from(self.fps_numerator) / f64::from(self.fps_denominator.max(1))
    }

    /// Track timescale and per-frame duration in timescale units. Small
    /// rates are scaled up so the timescale has enough resolution.
    fn timing(&self) -> (u32, u32) {
        let num = self.fps_numerator.max(1);
        let den = self.fps_denominator.max(1);
        let scale = 1000_u32.div_ceil(num).max(1);
        (num.saturating_mul(scale), den.saturating_mul(scale))
    }

    /// A target bitrate that keeps flat-colour animation crisp:
    /// about 0.2 bits per pixel per frame, at least 2 Mbit/s.
    fn bitrate(&self) -> u32 {
        let (w, h) = self.encoded_size();
        let bps = f64::from(w) * f64::from(h) * self.fps() * 0.2;
        bps.clamp(2_000_000.0, 100_000_000.0) as u32
    }
}

/// Writes frames to an MP4 (or MOV, by extension) file as they arrive. The file is deleted if the
/// encoder is dropped (or [`Self::abort`]ed) before [`Self::finish`].
pub struct Mp4Encoder {
    path: PathBuf,
    settings: VideoSettings,
    encoder: Encoder,
    writer: Option<mp4::Mp4Writer<BufWriter<File>>>,
    track_added: bool,
    frames: u64,
    finished: bool,
}

impl std::fmt::Debug for Mp4Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mp4Encoder")
            .field("path", &self.path)
            .field("settings", &self.settings)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl Mp4Encoder {
    /// Creates `path` and sets up the encoder.
    pub fn create(path: &Path, settings: VideoSettings) -> Result<Self, ExportError> {
        if settings.width == 0 || settings.height == 0 || settings.fps_numerator == 0 {
            return Err(ExportError::InvalidFrame(format!(
                "invalid video settings {settings:?}"
            )));
        }
        let fps = settings.fps() as f32;
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(settings.bitrate()))
            .max_frame_rate(FrameRate::from_hz(fps))
            .rate_control_mode(RateControlMode::Quality)
            // Every frame must produce a sample, or the timing drifts.
            .skip_frames(false)
            // A keyframe every ~2 seconds, so players can seek.
            .intra_frame_period(IntraFramePeriod::from_num_frames(
                (fps * 2.0).round().max(1.0) as u32,
            ))
            .vui(VuiConfig::bt709());
        let encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
            .map_err(|e| ExportError::Encoder(e.to_string()))?;

        let file = File::create(path).map_err(|source| ExportError::Io {
            path: path.to_owned(),
            source,
        })?;
        let mov = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("mov"));
        let (major, brands): (&str, &[&str]) = if mov {
            ("qt  ", &["qt  "])
        } else {
            ("isom", &["isom", "iso2", "avc1", "mp41"])
        };
        let brands = brands.iter().filter_map(|b| b.parse().ok()).collect();
        let config = mp4::Mp4Config {
            major_brand: major.parse()?,
            minor_version: 512,
            compatible_brands: brands,
            // Same as the track, so durations convert exactly.
            timescale: settings.timing().0,
        };
        let writer = mp4::Mp4Writer::write_start(BufWriter::new(file), &config);
        let writer = match writer {
            Ok(w) => w,
            Err(e) => {
                let _ = std::fs::remove_file(path);
                return Err(e.into());
            }
        };
        Ok(Self {
            path: path.to_owned(),
            settings,
            encoder,
            writer: Some(writer),
            track_added: false,
            frames: 0,
            finished: false,
        })
    }

    /// The output file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Encodes one frame. It must be `settings.width` x `settings.height`
    /// and opaque (alpha is ignored).
    pub fn push_frame(&mut self, image: &RgbaImage) -> Result<(), ExportError> {
        image.validate()?;
        if (image.width, image.height) != (self.settings.width, self.settings.height) {
            return Err(ExportError::InvalidFrame(format!(
                "frame is {}x{}, video is {}x{}",
                image.width, image.height, self.settings.width, self.settings.height
            )));
        }
        let padded = pad_to_even(image);
        let yuv = rgba_to_yuv420(&padded)
            .ok_or_else(|| ExportError::InvalidFrame("frame is not even-sized".into()))?;
        let (w, h) = (yuv.width as usize, yuv.height as usize);
        let buffer = YUVBuffer::from_vec(yuv.into_i420(), w, h);
        let (timescale, duration) = self.settings.timing();
        let millis = self.frames * 1000 * u64::from(duration) / u64::from(timescale);
        let bitstream = self
            .encoder
            .encode_at(&buffer, openh264::Timestamp::from_millis(millis))
            .map_err(|e| ExportError::Encoder(e.to_string()))?;

        let mut sps = None;
        let mut pps = None;
        let mut sample = Vec::new();
        for l in 0..bitstream.num_layers() {
            let Some(layer) = bitstream.layer(l) else {
                continue;
            };
            for n in 0..layer.nal_count() {
                let Some(nal) = layer.nal_unit(n).map(strip_start_code) else {
                    continue;
                };
                let Some(&header) = nal.first() else {
                    continue;
                };
                match header & 0x1f {
                    NAL_SPS => sps = Some(nal.to_vec()),
                    NAL_PPS => pps = Some(nal.to_vec()),
                    NAL_AUD => {}
                    _ => {
                        let len = u32::try_from(nal.len()).map_err(|_| {
                            ExportError::Encoder("NAL unit larger than 4 GiB".into())
                        })?;
                        sample.extend_from_slice(&len.to_be_bytes());
                        sample.extend_from_slice(nal);
                    }
                }
            }
        }
        let is_sync = matches!(bitstream.frame_type(), openh264::encoder::FrameType::IDR);
        let writer = self.writer.as_mut().ok_or(ExportError::Cancelled)?;

        if !self.track_added {
            let (Some(sps), Some(pps)) = (sps, pps) else {
                return Err(ExportError::Encoder(
                    "first frame has no SPS/PPS parameter sets".into(),
                ));
            };
            let (w, h) = self.settings.encoded_size();
            let too_big = || ExportError::InvalidFrame(format!("{w}x{h} is too large for MP4"));
            let avc = mp4::AvcConfig {
                width: u16::try_from(w).map_err(|_| too_big())?,
                height: u16::try_from(h).map_err(|_| too_big())?,
                seq_param_set: sps,
                pic_param_set: pps,
            };
            writer.add_track(&mp4::TrackConfig {
                track_type: mp4::TrackType::Video,
                timescale,
                language: "und".to_owned(),
                media_conf: mp4::MediaConfig::AvcConfig(avc),
            })?;
            self.track_added = true;
        }
        if sample.is_empty() {
            return Err(ExportError::Encoder(format!(
                "encoder produced no data for frame {}",
                self.frames + 1
            )));
        }
        writer.write_sample(
            1,
            &mp4::Mp4Sample {
                start_time: self.frames * u64::from(duration),
                duration,
                rendering_offset: 0,
                is_sync,
                bytes: sample.into(),
            },
        )?;
        self.frames += 1;
        Ok(())
    }

    /// Writes the index (`moov`) and closes the file.
    pub fn finish(mut self) -> Result<PathBuf, ExportError> {
        let result = self.finish_inner();
        if result.is_ok() {
            self.finished = true;
        }
        result.map(|()| self.path.clone())
    }

    fn finish_inner(&mut self) -> Result<(), ExportError> {
        if self.frames == 0 {
            return Err(ExportError::InvalidFrame("the video has no frames".into()));
        }
        let mut writer = self.writer.take().ok_or(ExportError::Cancelled)?;
        writer.write_end()?;
        let mut out = writer.into_writer();
        std::io::Write::flush(&mut out).map_err(|source| ExportError::Io {
            path: self.path.clone(),
            source,
        })?;
        out.get_ref().sync_all().map_err(|source| ExportError::Io {
            path: self.path.clone(),
            source,
        })
    }

    /// Stops and deletes the partial file.
    pub fn abort(self) {
        drop(self);
    }
}

impl Drop for Mp4Encoder {
    fn drop(&mut self) {
        if !self.finished {
            self.writer = None;
            if let Err(err) = std::fs::remove_file(&self.path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(path = %self.path.display(), %err, "could not delete partial video");
            }
        }
    }
}

/// The NAL unit without its Annex B start code (`00 00 01` or `00 00 00 01`).
fn strip_start_code(nal: &[u8]) -> &[u8] {
    let zeros = nal.iter().take_while(|&&b| b == 0).count();
    if zeros >= 2 && nal.get(zeros) == Some(&1) {
        &nal[zeros + 1..]
    } else {
        nal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yuv::yuv_to_rgb;
    use std::io::Read;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rusting-mp4-{name}-{}.mp4", std::process::id()))
    }

    fn settings(width: u32, height: u32) -> VideoSettings {
        VideoSettings {
            width,
            height,
            fps_numerator: 24,
            fps_denominator: 1,
        }
    }

    /// Top-level box types in file order.
    fn top_level_boxes(bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos + 8 <= bytes.len() {
            let size = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            out.push(String::from_utf8_lossy(&bytes[pos + 4..pos + 8]).into_owned());
            if size < 8 {
                break;
            }
            pos += size;
        }
        out
    }

    fn frame(width: u32, height: u32, i: u32) -> RgbaImage {
        // A red square moving across a white background.
        let mut img = RgbaImage::filled(width, height, [255, 255, 255, 255]);
        for y in 4..12 {
            for x in (i * 2)..(i * 2 + 8) {
                if x < width && y < height {
                    let p = ((y * width + x) * 4) as usize;
                    img.pixels[p..p + 4].copy_from_slice(&[220, 30, 30, 255]);
                }
            }
        }
        img
    }

    #[test]
    fn writes_a_valid_mp4_with_ftyp_and_moov() {
        let path = temp_path("valid");
        let mut enc = Mp4Encoder::create(&path, settings(64, 48)).unwrap();
        for i in 0..12 {
            enc.push_frame(&frame(64, 48, i)).unwrap();
        }
        assert_eq!(enc.frames(), 12);
        let out = enc.finish().unwrap();
        let mut bytes = Vec::new();
        File::open(&out).unwrap().read_to_end(&mut bytes).unwrap();
        let boxes = top_level_boxes(&bytes);
        assert_eq!(boxes.first().map(String::as_str), Some("ftyp"));
        assert!(boxes.iter().any(|b| b == "moov"), "{boxes:?}");
        assert!(boxes.iter().any(|b| b == "mdat"), "{boxes:?}");

        // Read it back: one H.264 track, 12 samples, 0.5 s long.
        let reader =
            mp4::Mp4Reader::read_header(std::io::Cursor::new(&bytes), bytes.len() as u64).unwrap();
        let track = reader.tracks().get(&1).unwrap();
        assert_eq!(track.media_type().unwrap(), mp4::MediaType::H264);
        assert_eq!((track.width(), track.height()), (64, 48));
        assert_eq!(track.sample_count(), 12);
        assert_eq!(reader.duration().as_millis(), 500);
        std::fs::remove_file(out).unwrap();
    }

    #[test]
    fn mov_path_gets_quicktime_brand() {
        let path = temp_path("brand").with_extension("mov");
        let mut enc = Mp4Encoder::create(&path, settings(16, 16)).unwrap();
        for _ in 0..3 {
            enc.push_frame(&RgbaImage::filled(16, 16, [0, 0, 0, 255]))
                .unwrap();
        }
        let out = enc.finish().unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[4..12], b"ftypqt  ");
        let reader =
            mp4::Mp4Reader::read_header(std::io::Cursor::new(&bytes), bytes.len() as u64).unwrap();
        assert_eq!(reader.tracks().get(&1).unwrap().sample_count(), 3);
        std::fs::remove_file(out).unwrap();
    }

    #[test]
    fn decoded_video_has_the_right_colours_and_odd_sizes_are_padded() {
        let path = temp_path("decode");
        let mut enc = Mp4Encoder::create(&path, settings(33, 17)).unwrap();
        let img = RgbaImage::filled(33, 17, [40, 120, 200, 255]);
        for _ in 0..3 {
            enc.push_frame(&img).unwrap();
        }
        let out = enc.finish().unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let mut reader =
            mp4::Mp4Reader::read_header(std::io::Cursor::new(&bytes), bytes.len() as u64).unwrap();
        let (sps, pps, w, h) = {
            let track = reader.tracks().get(&1).unwrap();
            (
                track.sequence_parameter_set().unwrap().to_vec(),
                track.picture_parameter_set().unwrap().to_vec(),
                track.width(),
                track.height(),
            )
        };
        assert_eq!((w, h), (34, 18));
        let sample = reader.read_sample(1, 1).unwrap().unwrap();
        // Back to Annex B for the decoder.
        let mut annex_b = Vec::new();
        for nal in [&sps, &pps] {
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(nal);
        }
        let mut data = &sample.bytes[..];
        while data.len() >= 4 {
            let len = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(&data[4..4 + len]);
            data = &data[4 + len..];
        }
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let decoded = match decoder.decode(&annex_b).unwrap() {
            Some(yuv) => {
                use openh264::formats::YUVSource;
                let (ys, us, vs) = yuv.strides();
                let (y, u, v) = (
                    yuv.y()[5 * ys + 5],
                    yuv.u()[2 * us + 2],
                    yuv.v()[2 * vs + 2],
                );
                yuv_to_rgb(y, u, v)
            }
            None => {
                let frames = decoder.flush_remaining().unwrap();
                use openh264::formats::YUVSource;
                let yuv = frames.first().unwrap();
                let (ys, us, vs) = yuv.strides();
                yuv_to_rgb(
                    yuv.y()[5 * ys + 5],
                    yuv.u()[2 * us + 2],
                    yuv.v()[2 * vs + 2],
                )
            }
        };
        for (got, want) in decoded.iter().zip([40u8, 120, 200]) {
            assert!(got.abs_diff(want) <= 6, "{decoded:?}");
        }
        std::fs::remove_file(out).unwrap();
    }

    #[test]
    fn dropping_before_finish_deletes_the_file() {
        let path = temp_path("abort");
        let mut enc = Mp4Encoder::create(&path, settings(16, 16)).unwrap();
        enc.push_frame(&RgbaImage::filled(16, 16, [0, 0, 0, 255]))
            .unwrap();
        assert!(path.exists());
        enc.abort();
        assert!(!path.exists());
    }

    #[test]
    fn wrong_frame_size_is_an_error() {
        let path = temp_path("size");
        let mut enc = Mp4Encoder::create(&path, settings(16, 16)).unwrap();
        assert!(matches!(
            enc.push_frame(&RgbaImage::filled(8, 8, [0; 4])),
            Err(ExportError::InvalidFrame(_))
        ));
        drop(enc);
        assert!(!path.exists());
    }

    #[test]
    fn timing_handles_whole_and_ntsc_rates() {
        let s = |n, d| VideoSettings {
            width: 2,
            height: 2,
            fps_numerator: n,
            fps_denominator: d,
        };
        assert_eq!(s(24, 1).timing(), (24 * 42, 42));
        assert_eq!(s(24000, 1001).timing(), (24000, 1001));
        assert_eq!(strip_start_code(&[0, 0, 0, 1, 0x67, 5]), &[0x67, 5]);
        assert_eq!(strip_start_code(&[0, 0, 1, 0x68]), &[0x68]);
    }
}
