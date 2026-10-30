//! WAV files: 32-bit float or 24-bit PCM out, and back in from most of what
//! audio programs write.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

const PCM: u16 = 1;
const IEEE_FLOAT: u16 = 3;
const EXTENSIBLE: u16 = 0xFFFE;

/// Writes 32-bit float samples, interleaved when there are several channels.
/// Float keeps everything above 0 dB intact, for mastering afterwards.
pub fn write(path: &Path, sample_rate: u32, channels: u16, samples: &[f32]) -> io::Result<()> {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    write_data(path, IEEE_FLOAT, sample_rate, channels, 32, &bytes)
}

/// Writes 24-bit PCM, clipping anything beyond full scale.
pub fn write_pcm24(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    samples: &[f32],
) -> io::Result<()> {
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|s| {
            let value = (f64::from(*s) * 8_388_608.0)
                .round()
                .clamp(-8_388_608.0, 8_388_607.0);
            (value as i32).to_le_bytes()[..3].to_vec()
        })
        .collect();
    write_data(path, PCM, sample_rate, channels, 24, &bytes)
}

fn write_data(
    path: &Path,
    format: u16,
    sample_rate: u32,
    channels: u16,
    bits: u16,
    data: &[u8],
) -> io::Result<()> {
    let data_len = u32::try_from(data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too long for a WAV file"))?;
    let frame_len = channels * bits / 8;
    let mut w = BufWriter::new(File::create(path)?);
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&format.to_le_bytes())?;
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&(sample_rate * u32::from(frame_len)).to_le_bytes())?;
    w.write_all(&frame_len.to_le_bytes())?;
    w.write_all(&bits.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    w.write_all(data)?;
    w.flush()
}

/// Audio read from a file: interleaved samples between -1 and 1.
pub struct Audio {
    pub sample_rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    /// One channel, in double precision for analysis.
    pub fn channel(&self, channel: usize) -> Vec<f64> {
        self.samples
            .iter()
            .skip(channel)
            .step_by(self.channels)
            .map(|&s| f64::from(s))
            .collect()
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// Reads PCM (16, 24 or 32-bit) or float (32 or 64-bit) WAV files, plain or
/// in the extensible format.
pub fn read(path: &Path) -> io::Result<Audio> {
    let bytes = fs::read(path)?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(invalid("not a WAV file"));
    }
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let (mut format, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32_at(at + 4) as usize;
        let body = at + 8..(at + 8 + size).min(bytes.len());
        match &bytes[at..at + 4] {
            b"fmt " if body.len() >= 16 => {
                let mut tag = u16_at(body.start);
                if tag == EXTENSIBLE && body.len() >= 26 {
                    tag = u16_at(body.start + 24);
                }
                let channels = usize::from(u16_at(body.start + 2));
                format = Some((
                    tag,
                    channels,
                    u32_at(body.start + 4),
                    u16_at(body.start + 14),
                ));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are aligned on two bytes.
        at += 8 + size + (size & 1);
    }
    let (tag, channels, sample_rate, bits) = format.ok_or_else(|| invalid("no format chunk"))?;
    let data = &bytes[data.ok_or_else(|| invalid("no data chunk"))?];
    if channels == 0 {
        return Err(invalid("no channels"));
    }
    let samples = match (tag, bits) {
        (PCM, 16) => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| f32::from(i16::from_le_bytes(b)) / 32_768.0)
            .collect(),
        (PCM, 24) => data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        (PCM, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| (f64::from(i32::from_le_bytes(b)) / 2_147_483_648.0) as f32)
            .collect(),
        (IEEE_FLOAT, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect(),
        (IEEE_FLOAT, 64) => data
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&b| f64::from_le_bytes(b) as f32)
            .collect(),
        _ => {
            return Err(invalid(&format!(
                "unsupported WAV format {tag} at {bits} bits"
            )));
        }
    };
    Ok(Audio {
        sample_rate,
        channels,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("audiochaos-studio-{name}.wav"))
    }

    #[test]
    fn float_files_come_back_exactly() {
        let path = temp("float");
        let samples = [0.5, -0.5, 1.5, 0.0];
        write(&path, 48_000, 2, &samples).unwrap();
        let bytes = fs::read(&path).unwrap();
        let audio = read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(bytes.len(), 44 + 16);
        assert_eq!(
            (audio.sample_rate, audio.channels, audio.frames()),
            (48_000, 2, 2)
        );
        assert_eq!(audio.samples, samples);
        assert_eq!(audio.channel(1), [-0.5, 0.0]);
    }

    #[test]
    fn pcm24_files_keep_24_bits_and_clip() {
        let path = temp("pcm24");
        write_pcm24(&path, 44_100, 1, &[0.25, -1.0, 2.0, -0.123_456_7]).unwrap();
        let audio = read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.samples[..2], [0.25, -1.0]);
        assert!(audio.samples[2] < 1.0 && audio.samples[2] > 0.999_999);
        assert!((audio.samples[3] + 0.123_456_7).abs() < 1e-6);
    }

    #[test]
    fn other_files_are_refused() {
        let path = temp("not-wav");
        fs::write(&path, b"RIFF....AVI LIST").unwrap();
        assert!(read(&path).is_err());
        fs::remove_file(&path).unwrap();
    }
}
