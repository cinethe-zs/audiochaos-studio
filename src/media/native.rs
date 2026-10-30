//! Loudness, mastering and spectrograms in Rust, without ffmpeg: EBU R128
//! measures from the ebur128 crate, MP3 from rusty_mp3, the rest here.

use std::f64::consts::PI;
use std::fs;
use std::io;
use std::path::Path;

use ebur128::{EbuR128, Mode};
use rusty_mp3::{Mp3Encoder, Mp3EncoderConfig};

use super::{CEILING_DB, Loudness, Picture};
use crate::{dsp, wav};

/// MP3s are written at a constant bitrate, where rusty_mp3 comes closest to
/// LAME, and at most 256 kbps, where it still uses its bit reservoir.
const MP3_KBPS: u32 = 256;

/// Half the width of the interpolator that finds the peaks between samples.
const TAPS: i64 = 8;

fn read(path: &Path) -> Result<wav::Audio, String> {
    wav::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

pub fn loudness(path: &Path) -> Result<Loudness, String> {
    let audio = read(path)?;
    measure(&audio.samples, audio.channels, audio.sample_rate)
}

fn measure(samples: &[f32], channels: usize, sample_rate: u32) -> Result<Loudness, String> {
    let fail = |e: ebur128::Error| format!("loudness: {e}");
    let mut meter = EbuR128::new(
        channels as u32,
        sample_rate,
        Mode::I | Mode::LRA | Mode::TRUE_PEAK,
    )
    .map_err(fail)?;
    meter.add_frames_f32(samples).map_err(fail)?;
    let mut peak: f64 = 0.0;
    for channel in 0..channels {
        peak = peak.max(meter.true_peak(channel as u32).map_err(fail)?);
    }
    Ok(Loudness {
        integrated: meter.loudness_global().map_err(fail)?,
        true_peak: 20.0 * peak.log10(),
        range: meter.loudness_range().map_err(fail)?,
    })
}

/// Gain to `lufs` and a true-peak limiter at the ceiling; writes a 24-bit
/// WAV and an MP3. Returns the measures before and after.
pub fn master(
    mix: &Path,
    wav_out: &Path,
    mp3: &Path,
    lufs: f64,
) -> Result<(Loudness, Loudness), String> {
    let audio = read(mix)?;
    let before = measure(&audio.samples, audio.channels, audio.sample_rate)?;
    let peaks = true_peaks(&audio);
    // The limiter takes some loudness off the peaks: correct once for it.
    let mut gain = lufs - before.integrated;
    let mut mastered = Vec::new();
    for _ in 0..2 {
        mastered = limit(&audio, &peaks, gain);
        gain += lufs - measure(&mastered, audio.channels, audio.sample_rate)?.integrated;
    }
    let channels = audio.channels as u16;
    wav::write_pcm24(wav_out, audio.sample_rate, channels, &mastered)
        .map_err(|e| format!("cannot write {}: {e}", wav_out.display()))?;
    encode_mp3(&mastered, channels, audio.sample_rate, mp3)?;
    Ok((before, loudness(wav_out)?))
}

/// A windowed sinc that reads a signal a fraction of a sample after a sample.
fn interpolator(fraction: f64) -> Vec<f64> {
    (-(TAPS - 1)..=TAPS)
        .map(|k| {
            let t = fraction - k as f64;
            let sinc = if t == 0.0 {
                1.0
            } else {
                (PI * t).sin() / (PI * t)
            };
            sinc * 0.5 * (1.0 + (PI * t / TAPS as f64).cos())
        })
        .collect()
}

/// The loudest point of each frame, from the frame up to the next one, in
/// any channel: samples, and three points between them.
fn true_peaks(audio: &wav::Audio) -> Vec<f64> {
    let interpolators: Vec<Vec<f64>> = [0.25, 0.5, 0.75].into_iter().map(interpolator).collect();
    let frames = audio.frames();
    let mut peaks = vec![0.0f64; frames];
    for channel in 0..audio.channels {
        let x = audio.channel(channel);
        for (n, peak) in peaks.iter_mut().enumerate() {
            let mut loudest = x[n].abs();
            for weights in &interpolators {
                let first = n as i64 - (TAPS - 1);
                let value: f64 = weights
                    .iter()
                    .enumerate()
                    .filter_map(|(j, w)| {
                        x.get(usize::try_from(first + j as i64).ok()?)
                            .map(|s| s * w)
                    })
                    .sum();
                loudest = loudest.max(value.abs());
            }
            *peak = peak.max(loudest);
        }
    }
    peaks
}

/// The mix raised by `gain_db` and held under the ceiling: the gain comes
/// down over the millisecond before each peak and recovers over 50 ms.
fn limit(audio: &wav::Audio, peaks: &[f64], gain_db: f64) -> Vec<f32> {
    let gain = 10f64.powf(gain_db / 20.0);
    let ceiling = 10f64.powf(CEILING_DB / 20.0);
    let rate = f64::from(audio.sample_rate);
    let ahead = ((0.001 * rate).round() as usize).max(1);
    let release = 1.0 - (-1.0 / (0.05 * rate)).exp();
    // What each frame needs on its own.
    let need: Vec<f64> = peaks
        .iter()
        .map(|&p| {
            if p * gain > ceiling {
                ceiling / (p * gain)
            } else {
                1.0
            }
        })
        .collect();
    // Known `ahead` frames early, and recovering slowly...
    let mut held = Vec::with_capacity(need.len());
    let mut level = 1.0f64;
    for n in 0..need.len() {
        let soonest = need[n..(n + ahead).min(need.len())]
            .iter()
            .copied()
            .fold(1.0, f64::min);
        level = (level + (1.0 - level) * release).min(soonest);
        held.push(level);
    }
    // ...then averaged over `ahead` frames: every frame of the average knew
    // about this frame's peak, so it stays under what this frame needs.
    let mut out = Vec::with_capacity(audio.samples.len());
    let mut sum = 0.0;
    for (n, frame) in audio.samples.chunks_exact(audio.channels).enumerate() {
        sum += held[n];
        if n >= ahead {
            sum -= held[n - ahead];
        }
        let g = gain * sum / (n + 1).min(ahead) as f64;
        out.extend(frame.iter().map(|&s| (f64::from(s) * g) as f32));
    }
    out
}

fn encode_mp3(samples: &[f32], channels: u16, sample_rate: u32, path: &Path) -> Result<(), String> {
    let mut encoder = Mp3Encoder::new(Mp3EncoderConfig {
        bitrate_kbps: MP3_KBPS,
        vbr_quality: None,
    });
    encoder
        .push_pcm_f32(samples, channels, sample_rate)
        .map_err(|e| format!("MP3: {e:?}"))?;
    encoder.finish();
    let mut bytes = Vec::new();
    while let Ok(packet) = encoder.next_packet() {
        bytes.extend_from_slice(&packet);
    }
    fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

const WIDTH: usize = 1400;
const HEIGHT: usize = 500;
const WINDOW: usize = 4096;
const LOWEST_HZ: f64 = 20.0;
const HIGHEST_HZ: f64 = 20_000.0;

/// Time across, frequency up on a log scale from 20 Hz to 20 kHz, level in
/// colour from -120 dB (black) to 0 dB (white). Faint lines mark 100 Hz,
/// 1 kHz and 10 kHz, and the starts of sections.
pub fn spectrogram(picture: &Picture) -> Result<(), String> {
    let audio = read(picture.audio)?;
    let rate = f64::from(audio.sample_rate);
    let mono: Vec<f64> = audio
        .samples
        .chunks_exact(audio.channels)
        .map(|frame| frame.iter().map(|&s| f64::from(s)).sum::<f64>() / audio.channels as f64)
        .collect();
    let start = ((picture.from * rate) as usize).min(mono.len());
    let end = picture.length.map_or(mono.len(), |length| {
        (start + (length * rate) as usize).min(mono.len())
    });
    let part = &mono[start..end];
    let window: Vec<f64> = (0..WINDOW)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / WINDOW as f64).cos())
        .collect();
    let row_hz =
        |y: f64| (HIGHEST_HZ.ln() - (HIGHEST_HZ / LOWEST_HZ).ln() * y / HEIGHT as f64).exp();
    let bin = |hz: f64| hz * WINDOW as f64 / rate;
    let mut rgb = vec![0u8; WIDTH * HEIGHT * 3];
    for x in 0..WIDTH {
        let centre = (part.len() * (2 * x + 1) / (2 * WIDTH)) as i64;
        let mut re: Vec<f64> = (0..WINDOW)
            .map(|i| {
                let at = centre + i as i64 - WINDOW as i64 / 2;
                usize::try_from(at)
                    .ok()
                    .and_then(|at| part.get(at))
                    .map_or(0.0, |s| s * window[i])
            })
            .collect();
        let mut im = vec![0.0; WINDOW];
        dsp::fft(&mut re, &mut im);
        // A full-scale sine reads 0 dB through the Hann window.
        let magnitude: Vec<f64> = (0..=WINDOW / 2)
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() * 4.0 / WINDOW as f64)
            .collect();
        let at = |position: f64| {
            let k = (position as usize).min(WINDOW / 2 - 1);
            let fraction = (position - k as f64).clamp(0.0, 1.0);
            magnitude[k] * (1.0 - fraction) + magnitude[k + 1] * fraction
        };
        for y in 0..HEIGHT {
            let (top, bottom) = (bin(row_hz(y as f64)), bin(row_hz(y as f64 + 1.0)));
            // Rows narrower than a bin read between bins; wider ones keep
            // their loudest bin.
            let level = if top - bottom < 1.0 {
                at((top + bottom) / 2.0)
            } else {
                (bottom.ceil() as usize..=(top as usize).min(WINDOW / 2))
                    .map(|k| magnitude[k])
                    .fold(0.0, f64::max)
            };
            let pixel = (y * WIDTH + x) * 3;
            rgb[pixel..pixel + 3].copy_from_slice(&colour(20.0 * level.max(1e-9).log10()));
        }
    }
    let seconds = part.len() as f64 / rate;
    for hz in [100.0, 1000.0, 10_000.0] {
        let y = ((HIGHEST_HZ / hz).ln() / (HIGHEST_HZ / LOWEST_HZ).ln() * HEIGHT as f64) as usize;
        for x in (0..WIDTH).step_by(4) {
            brighten(&mut rgb, x, y.min(HEIGHT - 1), 60);
        }
    }
    for &mark in picture.marks {
        let x = (mark - picture.from) / seconds * WIDTH as f64;
        if (0.0..WIDTH as f64).contains(&x) {
            for y in 0..HEIGHT {
                brighten(&mut rgb, x as usize, y, 110);
            }
        }
    }
    write_png(picture.out, WIDTH, HEIGHT, &rgb)
        .map_err(|e| format!("cannot write {}: {e}", picture.out.display()))
}

fn brighten(rgb: &mut [u8], x: usize, y: usize, amount: u8) {
    let pixel = (y * WIDTH + x) * 3;
    for channel in &mut rgb[pixel..pixel + 3] {
        *channel = channel.saturating_add(amount);
    }
}

/// Black, through violet, red, orange and yellow, to white.
fn colour(db: f64) -> [u8; 3] {
    const STOPS: [(f64, [f64; 3]); 7] = [
        (0.0, [0.0, 0.0, 0.0]),
        (0.25, [25.0, 0.0, 70.0]),
        (0.45, [120.0, 0.0, 120.0]),
        (0.6, [215.0, 25.0, 40.0]),
        (0.75, [250.0, 125.0, 0.0]),
        (0.9, [255.0, 225.0, 40.0]),
        (1.0, [255.0, 255.0, 255.0]),
    ];
    let t = ((db + 120.0) / 120.0).clamp(0.0, 1.0);
    let upper = STOPS
        .iter()
        .position(|&(stop, _)| stop >= t)
        .unwrap_or(STOPS.len() - 1)
        .max(1);
    let ((t0, c0), (t1, c1)) = (STOPS[upper - 1], STOPS[upper]);
    let f = (t - t0) / (t1 - t0);
    [0usize, 1, 2].map(|i| (c0[i] + (c1[i] - c0[i]) * f).round() as u8)
}

/// Writes an RGB picture as a PNG, its pixels stored without compression.
fn write_png(path: &Path, width: usize, height: usize, rgb: &[u8]) -> io::Result<()> {
    let mut raw = Vec::with_capacity((width * 3 + 1) * height);
    for row in rgb.chunks_exact(width * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        let length = block.len() as u16;
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    fs::write(path, png)
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = png.len();
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let crc = crc32(&png[start..]);
    png.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amplitude: f32, seconds: f32) -> wav::Audio {
        let frames = (48_000.0 * seconds) as usize;
        let samples = (0..frames)
            .flat_map(|n| {
                let s =
                    amplitude * (2.0 * std::f32::consts::PI * 997.0 * n as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect();
        wav::Audio {
            sample_rate: 48_000,
            channels: 2,
            samples,
        }
    }

    #[test]
    fn the_limiter_holds_the_peaks_under_the_ceiling() {
        let audio = sine(0.9, 1.0);
        let peaks = true_peaks(&audio);
        let limited = limit(&audio, &peaks, 6.0);
        let ceiling = 10f32.powf(CEILING_DB as f32 / 20.0);
        let loudest = limited.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(loudest <= ceiling + 1e-4, "{loudest}");
        assert!(
            loudest > 0.85,
            "the limiter only takes off what it must: {loudest}"
        );
        let measured = measure(&limited, 2, 48_000).unwrap();
        assert!(
            measured.true_peak <= CEILING_DB + 0.1,
            "{}",
            measured.true_peak
        );
    }

    #[test]
    fn mp3s_decode_back() {
        let audio = sine(0.5, 0.5);
        let path = std::env::temp_dir().join("audiochaos-studio-test.mp3");
        encode_mp3(&audio.samples, 2, 48_000, &path).unwrap();
        let mut decoder = rusty_mp3::Mp3Decoder::new();
        decoder.push(&fs::read(&path).unwrap());
        decoder.flush();
        fs::remove_file(&path).unwrap();
        let mut decoded = Vec::new();
        while let Ok(frame) = decoder.next_frame() {
            decoded.extend(frame.samples);
        }
        let loudest = decoded.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            decoded.len() >= audio.samples.len() && (loudest - 0.5).abs() < 0.05,
            "{loudest}"
        );
    }

    #[test]
    fn pictures_are_pngs() {
        let path = std::env::temp_dir().join("audiochaos-studio-test.png");
        write_png(&path, 2, 1, &[255, 0, 0, 0, 0, 255]).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(colour(-120.0), [0, 0, 0]);
        assert_eq!(colour(0.0), [255, 255, 255]);
    }
}
