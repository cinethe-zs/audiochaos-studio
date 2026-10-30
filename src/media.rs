//! What the studio asks of ffmpeg: loudness and true peak, mastering to WAV
//! and MP3, spectrogram pictures. The ffmpeg program does it by default; the
//! `native` feature does it in Rust instead, for a machine without ffmpeg.

use std::path::Path;

#[cfg(not(feature = "native"))]
mod ffmpeg;
#[cfg(feature = "native")]
mod native;

#[cfg(not(feature = "native"))]
pub use ffmpeg::{loudness, master, spectrogram};
#[cfg(feature = "native")]
pub use native::{loudness, master, spectrogram};

/// Masters peak this far under full scale.
pub const CEILING_DB: f64 = -1.0;

/// EBU R128 measures of a file.
#[derive(Clone, Copy, Debug)]
pub struct Loudness {
    /// Integrated loudness, in LUFS.
    pub integrated: f64,
    /// Highest peak, between samples included, in dBTP.
    pub true_peak: f64,
    /// Loudness range, in LU.
    pub range: f64,
}

/// A spectrogram to draw from part of an audio file.
pub struct Picture<'a> {
    pub audio: &'a Path,
    pub out: &'a Path,
    /// Where the part starts, and how long it lasts (to the end if not
    /// given), in seconds.
    pub from: f64,
    pub length: Option<f64>,
    /// Where sections start, in seconds, for the drawings that mark them.
    pub marks: &'a [f64],
}
