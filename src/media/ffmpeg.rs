//! Loudness, mastering and spectrograms through the ffmpeg program.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use super::{CEILING_DB, Loudness, Picture};

fn ffmpeg(args: Vec<OsString>) -> Result<String, String> {
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-y"])
        .args(args)
        .output()
        .map_err(|e| {
            format!(
                "cannot run ffmpeg ({e}): install it, or build the studio with --features native"
            )
        })?;
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        Ok(log)
    } else {
        Err(format!("ffmpeg failed:\n{log}"))
    }
}

/// The last value labelled `label` in ffmpeg's EBU R128 summary.
fn summary(log: &str, label: &str) -> Result<f64, String> {
    log.lines()
        .rev()
        .find_map(|line| {
            let number = line.trim().strip_prefix(label)?.split_whitespace().next()?;
            if number == "-inf" {
                Some(f64::NEG_INFINITY)
            } else {
                number.parse().ok()
            }
        })
        .ok_or_else(|| format!("no {label} in ffmpeg's loudness summary"))
}

pub fn loudness(path: &Path) -> Result<Loudness, String> {
    let log = ffmpeg(vec![
        "-i".into(),
        path.into(),
        "-af".into(),
        "ebur128=peak=true:framelog=quiet".into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ])?;
    Ok(Loudness {
        integrated: summary(&log, "I:")?,
        true_peak: summary(&log, "Peak:")?,
        range: summary(&log, "LRA:")?,
    })
}

/// Gain to `lufs` and a limiter at the ceiling, run four times oversampled
/// to catch the peaks between samples; writes a 24-bit WAV and a 320 kbps
/// MP3. Returns the measures before and after.
pub fn master(
    mix: &Path,
    wav: &Path,
    mp3: &Path,
    lufs: f64,
) -> Result<(Loudness, Loudness), String> {
    let before = loudness(mix)?;
    let mut after = before;
    // The limiter takes some loudness off the peaks: correct once for it.
    let mut gain = lufs - before.integrated;
    for _ in 0..2 {
        let chain = format!(
            "volume={gain:.2}dB,aresample=192000,alimiter=limit={:.4}:attack=1:release=50:level=0,aresample=48000",
            10f64.powf(CEILING_DB / 20.0)
        );
        ffmpeg(vec![
            "-i".into(),
            mix.into(),
            "-af".into(),
            chain.into(),
            "-c:a".into(),
            "pcm_s24le".into(),
            wav.into(),
        ])?;
        after = loudness(wav)?;
        gain += lufs - after.integrated;
    }
    ffmpeg(vec![
        "-i".into(),
        wav.into(),
        "-c:a".into(),
        "libmp3lame".into(),
        "-b:a".into(),
        "320k".into(),
        mp3.into(),
    ])?;
    Ok((before, after))
}

pub fn spectrogram(picture: &Picture) -> Result<(), String> {
    let mut args: Vec<OsString> = vec!["-ss".into(), format!("{:.3}", picture.from).into()];
    if let Some(length) = picture.length {
        args.extend(["-t".into(), format!("{length:.3}").into()]);
    }
    args.extend([
        "-i".into(),
        picture.audio.into(),
        "-lavfi".into(),
        "showspectrumpic=s=1400x500:mode=combined:color=intensity:scale=log:fscale=log:legend=1"
            .into(),
        "-frames:v".into(),
        "1".into(),
        picture.out.into(),
    ]);
    ffmpeg(args).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_loudness_summary() {
        let log = "[Parsed_ebur128_0] Summary:\n\n  Integrated loudness:\n    I:         -12.0 LUFS\n    \
                   Threshold: -22.5 LUFS\n\n  Loudness range:\n    LRA:         6.4 LU\n    \
                   LRA low:   -15.3 LUFS\n\n  True peak:\n    Peak:       -0.9 dBFS\n";
        assert_eq!(summary(log, "I:"), Ok(-12.0));
        assert_eq!(summary(log, "LRA:"), Ok(6.4));
        assert_eq!(summary(log, "Peak:"), Ok(-0.9));
        assert!(summary("nothing", "I:").is_err());
    }
}
