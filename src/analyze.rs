//! Looking at a render without listening to it: how loud each stem plays in
//! each section, clicks where cables are pulled, the master's loudness,
//! stereo and spectral balance, and spectrograms to look at.

use std::fs;
use std::path::Path;

use crate::dsp;
use crate::media::{self, Picture};
use crate::score::{Score, file_name};
use crate::wav;

const OCTAVES: [(f64, f64); 10] = [
    (20.0, 40.0),
    (40.0, 80.0),
    (80.0, 160.0),
    (160.0, 320.0),
    (320.0, 640.0),
    (640.0, 1280.0),
    (1280.0, 2560.0),
    (2560.0, 5120.0),
    (5120.0, 10240.0),
    (10240.0, 20000.0),
];

fn read(path: &Path) -> Result<wav::Audio, String> {
    let audio = wav::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if audio.sample_rate != 48_000 {
        return Err(format!("{} is not at 48 kHz", path.display()));
    }
    Ok(audio)
}

/// The momentary loudness windows that fit between two times, in seconds.
fn within(levels: &[f64], start: f64, stop: f64) -> &[f64] {
    let first = ((start * 10.0) as usize).min(levels.len());
    let last = ((stop * 10.0) as usize)
        .saturating_sub(3)
        .max(first + 1)
        .min(levels.len());
    &levels[first.min(last)..last]
}

/// How many cable pulls click, and how loud the loudest is (dBFS): a click
/// is a jump in the second difference well above the tail just before.
fn clicks(signal: &[f64], pulls: &[usize]) -> (usize, Option<f64>) {
    let curvature: Vec<f64> = signal
        .windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
        .collect();
    let range =
        |from: usize, to: usize| &curvature[from.min(curvature.len())..to.min(curvature.len())];
    let jumps: Vec<f64> = pulls
        .iter()
        .filter_map(|&at| {
            let after = range(at.saturating_sub(96), at + 320);
            let before = range(at.saturating_sub(1000), at.saturating_sub(150).max(1));
            if after.is_empty() || before.is_empty() {
                return None;
            }
            let jump = after.iter().copied().fold(0.0, f64::max);
            let tail = (before.iter().map(|c| c * c).sum::<f64>() / before.len() as f64).sqrt();
            (jump > 8.0 * tail && jump > 10f64.powf(-70.0 / 20.0)).then_some(jump)
        })
        .collect();
    let loudest = jumps
        .iter()
        .copied()
        .reduce(f64::max)
        .map(|j| 20.0 * j.log10());
    (jumps.len(), loudest)
}

/// Prints the analysis of `renders/<name>/` and draws its spectrograms,
/// with a closer one from bar `zoom` on, `zoom_bars` long, if asked.
pub fn analyze(name: &str, out: &Path, zoom: Option<f64>, zoom_bars: f64) -> Result<(), String> {
    let path = out.join("track.json");
    let json =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let score = Score::from_json(&json)?;
    let beat = score.bpm.map_or(1.0, |bpm| 60.0 / bpm);
    let bar = score.meta["beats_per_bar"].as_f64().unwrap_or(4.0) * beat;
    let end = score.length * beat;
    let mut markers: Vec<(String, f64)> = score.meta["markers"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(name, at)| Some((name.clone(), at.as_f64()? * beat)))
                .collect()
        })
        .unwrap_or_default();
    markers.sort_by(|a, b| a.1.total_cmp(&b.1));
    if markers.is_empty() {
        markers.push(("all".into(), 0.0));
    }
    let sections: Vec<(&str, f64, f64)> = markers
        .iter()
        .enumerate()
        .map(|(i, (name, start))| {
            (
                name.as_str(),
                *start,
                markers.get(i + 1).map_or(end, |next| next.1),
            )
        })
        .collect();

    println!("Loudness while playing (LUFS, loudest 10% of 400 ms windows, after gain):");
    let header: String = sections
        .iter()
        .map(|(name, ..)| format!("{:>9}", truncate(name, 8)))
        .collect();
    println!("{:13}{header}   cable pulls", "");
    for stem in &score.stems {
        let audio = read(&out.join("stems").join(file_name(&stem.name)))?;
        let gain = 10f64.powf(f64::from(stem.gain_db) / 20.0);
        let signal: Vec<f64> = audio.channel(0).iter().map(|s| s * gain).collect();
        let levels = dsp::momentary(std::slice::from_ref(&signal));
        let row: String = sections
            .iter()
            .map(|&(_, start, stop)| {
                let part = within(&levels, start, stop);
                let level = if part.is_empty() {
                    -99.0
                } else {
                    dsp::percentile(part, 90.0)
                };
                if level > -60.0 {
                    format!("{level:9.1}")
                } else {
                    format!("{:>9}", ".")
                }
            })
            .collect();
        let pulls: Vec<usize> = stem
            .events
            .iter()
            .filter(|e| e.disconnect.is_some())
            .map(|e| (e.at * beat * 48_000.0) as usize)
            .filter(|&at| at < signal.len())
            .collect();
        let report = match clicks(&signal, &pulls) {
            _ if pulls.is_empty() => String::new(),
            (0, _) => format!("   {} clean", pulls.len()),
            (count, loudest) => format!(
                "   {count} of {} click, loudest {:.0} dBFS",
                pulls.len(),
                loudest.unwrap_or(0.0)
            ),
        };
        println!("{:13}{row}{report}", truncate(&stem.name, 12));
    }

    let mut master = out.join(format!("{name}.wav"));
    if !master.is_file() {
        master = out.join("mix.wav");
    }
    let audio = read(&master)?;
    let (left, right) = (audio.channel(0), audio.channel(audio.channels.min(2) - 1));
    let levels = dsp::momentary(&[left.clone(), right.clone()]);
    println!(
        "\n{}, median loudness per section (LUFS):",
        master.file_name().unwrap_or_default().to_string_lossy()
    );
    let medians: Vec<String> = sections
        .iter()
        .map(|&(name, start, stop)| {
            let part = within(&levels, start, stop);
            if part.is_empty() {
                format!("{name} -")
            } else {
                format!("{name} {:.1}", dsp::percentile(part, 50.0))
            }
        })
        .collect();
    println!("  {}", medians.join("  "));
    let loudness = media::loudness(&master)?;
    let mono: Vec<f64> = left
        .iter()
        .zip(&right)
        .map(|(l, r)| (l + r) / 2.0)
        .collect();
    let dc = audio.samples.iter().map(|&s| f64::from(s)).sum::<f64>() / audio.samples.len() as f64;
    println!(
        "  integrated {:.1} LUFS, true peak {:.1} dBTP, range {:.1} LU, L/R correlation {:.2}, DC {:.4}",
        loudness.integrated,
        loudness.true_peak,
        loudness.range,
        correlation(&left, &right),
        dc.abs()
    );
    let spectrum = dsp::welch(&mono, 8192);
    let total: f64 = spectrum.iter().sum();
    let shares: Vec<String> = OCTAVES
        .iter()
        .map(|&(low, high)| {
            let band: f64 = (0..spectrum.len())
                .filter(|&k| (low..high).contains(&(k as f64 * 48_000.0 / 8192.0)))
                .map(|k| spectrum[k])
                .sum();
            format!(
                "{low}-{high}: {:.0}",
                10.0 * (band / total).max(1e-12).log10()
            )
        })
        .collect();
    println!(
        "  energy per octave, dB of the total (Hz): {}",
        shares.join(", ")
    );

    let marks: Vec<f64> = markers.iter().map(|m| m.1).collect();
    let mut pictures = vec![(out.join("spectrum.png"), 0.0, None)];
    if let Some(zoom) = zoom {
        pictures.push((
            out.join(format!("zoom-bar{zoom}.png")),
            zoom * bar,
            Some(zoom_bars * bar),
        ));
    }
    for (picture, from, length) in pictures {
        media::spectrogram(&Picture {
            audio: &master,
            out: &picture,
            from,
            length,
            marks: &marks,
        })?;
        println!("wrote {}", picture.display());
    }
    Ok(())
}

fn truncate(text: &str, width: usize) -> &str {
    text.char_indices()
        .nth(width)
        .map_or(text, |(at, _)| &text[..at])
}

fn correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (mean_a, mean_b) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x - mean_a, y - mean_b);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    ab / (aa * bb).sqrt().max(1e-12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_stand_out_of_the_tail() {
        // A smooth decay, then a step where the cable is pulled.
        let mut signal: Vec<f64> = (0..4000).map(|n| (-(n as f64) / 400.0).exp()).collect();
        for s in &mut signal[2000..] {
            *s = 0.0;
        }
        assert_eq!(clicks(&signal, &[2000]).0, 1);
        assert_eq!(clicks(&signal, &[3500]).0, 0, "silence does not click");
    }

    #[test]
    fn sections_keep_their_whole_windows() {
        let levels: Vec<f64> = (0..100).map(f64::from).collect();
        assert_eq!(within(&levels, 1.0, 2.0), &levels[10..17]);
        assert!(within(&levels, 20.0, 30.0).is_empty());
    }
}
