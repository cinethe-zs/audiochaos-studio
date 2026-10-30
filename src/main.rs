//! The studio: `cargo studio <command>` composes the tracks under tracks/,
//! renders them on audiochaos racks, masters and analyzes them.

mod tracks;

#[cfg(test)]
#[path = "template.rs"]
mod template;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;
use std::{env, fs};

use audiochaos::rack;
use audiochaos_studio::analyze::analyze;
use audiochaos_studio::media;
use audiochaos_studio::score::{Score, file_name};
use audiochaos_studio::wav;

const USAGE: &str = "usage: cargo studio <command>

  new <name>                       start tracks/<name>/compose.rs from a template
  build <name> [--bars n] [--no-master]
                                   compose, render and master into renders/<name>/
  analyze <name> [--zoom bar] [--bars n]
                                   loudness of each stem per section, clicks,
                                   balance of the master, spectrograms
  sound <patch.json | preset> --seconds s [-o file.wav]
                                   render one patch on its own
  master <file.wav> [--lufs l]     master a file to a 24-bit WAV and an MP3
  render <score.json> [-o mix.wav] [--stems dir] [--length t]
                                   render a score written by hand";

/// The studio's folder: tracks/ and renders/ are there.
const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const TEMPLATE: &str = include_str!("template.rs");
const MASTER_LUFS: f64 = -12.0;

fn main() -> ExitCode {
    // Returning the error from `main` would print it with `Debug`, quotes and
    // escapes included.
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A command's arguments: options are taken out by name, the rest is
/// positional.
struct Args(Vec<String>);

impl Args {
    fn flag(&mut self, name: &str) -> bool {
        let found = self.0.iter().position(|arg| arg == name);
        if let Some(at) = found {
            self.0.remove(at);
        }
        found.is_some()
    }

    fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        let Some(at) = self.0.iter().position(|arg| arg == name) else {
            return Ok(None);
        };
        if at + 1 == self.0.len() {
            return Err(format!("{name} needs a value"));
        }
        self.0.remove(at);
        Ok(Some(self.0.remove(at)))
    }

    fn number(&mut self, name: &str) -> Result<Option<f64>, String> {
        self.value(name)?
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| format!("{name} takes a number, not {value:?}"))
            })
            .transpose()
    }

    /// The one positional argument left.
    fn one(self) -> Result<String, String> {
        if let Some(option) = self.0.iter().find(|arg| arg.starts_with('-')) {
            return Err(format!("unknown option {option}"));
        }
        match <[String; 1]>::try_from(self.0) {
            Ok([arg]) => Ok(arg),
            Err(args) => Err(format!("expected one argument, got {}", args.len())),
        }
    }
}

fn run(mut args: Vec<String>) -> Result<(), Box<dyn Error>> {
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}\n\ntracks: {}", track_names());
        return Ok(());
    }
    let command = args.remove(0);
    let mut args = Args(args);
    match command.as_str() {
        "new" => new(&args.one()?),
        "build" => {
            let bars = args.number("--bars")?;
            let master = !args.flag("--no-master");
            build(&args.one()?, bars, master)
        }
        "analyze" => {
            let zoom = args.number("--zoom")?;
            let bars = args.number("--bars")?.unwrap_or(2.0);
            let name = args.one()?;
            Ok(analyze(&name, &renders().join(&name), zoom, bars)?)
        }
        "sound" => {
            let seconds = args.number("--seconds")?.ok_or("sound needs --seconds")?;
            let output = args.value("-o")?;
            sound(&args.one()?, seconds, output)
        }
        "master" => {
            let lufs = args.number("--lufs")?.unwrap_or(MASTER_LUFS);
            let file = PathBuf::from(args.one()?);
            let named = |suffix: &str| {
                let stem = file.file_stem().unwrap_or_default().to_string_lossy();
                file.with_file_name(format!("{stem}-master.{suffix}"))
            };
            master(&file, &named("wav"), &named("mp3"), lufs)
        }
        "render" => {
            let output = args.value("-o")?;
            let stems = args.value("--stems")?;
            let length = args.number("--length")?;
            let path = PathBuf::from(args.one()?);
            let json = fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let mut score = Score::from_json(&json)?;
            if let Some(length) = length {
                score.length = length;
            }
            let mix = output.map_or_else(|| path.with_extension("wav"), PathBuf::from);
            let dir = path.parent().unwrap_or(Path::new(""));
            render(&score, dir, &mix, stems.as_deref().map(Path::new))
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}").into()),
    }
}

fn tracks_dir() -> PathBuf {
    Path::new(ROOT).join("tracks")
}

fn renders() -> PathBuf {
    Path::new(ROOT).join("renders")
}

fn track_names() -> String {
    tracks::TRACKS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

fn new(name: &str) -> Result<(), Box<dyn Error>> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("{name:?}: a track name takes letters, digits, - and _").into());
    }
    let folder = tracks_dir().join(name);
    if folder.exists() {
        return Err(format!("{} already exists", folder.display()).into());
    }
    fs::create_dir_all(&folder)?;
    let path = folder.join("compose.rs");
    fs::write(&path, TEMPLATE.replace("NAME", name))?;
    println!("wrote {}\nnext: cargo studio build {name}", path.display());
    Ok(())
}

/// Composes, renders (mix and stems) and masters a track into renders/<name>/.
fn build(name: &str, bars: Option<f64>, master_it: bool) -> Result<(), Box<dyn Error>> {
    let compose = tracks::find(name).ok_or_else(|| {
        format!(
            "no track named {name:?} (tracks: {}); start one with: cargo studio new {name}",
            track_names()
        )
    })?;
    let track = compose();
    let out = renders().join(name);
    let path = track.write(&out, &tracks_dir().join(name))?;
    let mut score = Score::from_json(&fs::read_to_string(&path)?)?;
    if let Some(bars) = bars {
        score.length = score.length.min(bars * track.clock.beats_per_bar);
    }
    let stems = out.join("stems");
    fs::create_dir_all(&stems)?;
    for entry in fs::read_dir(&stems)? {
        let old = entry?.path();
        if old.extension().is_some_and(|e| e == "wav") {
            fs::remove_file(old)?;
        }
    }
    let mix = out.join("mix.wav");
    render(&score, &out, &mix, Some(&stems))?;
    if master_it {
        master(
            &mix,
            &out.join(format!("{name}.wav")),
            &out.join(format!("{name}.mp3")),
            MASTER_LUFS,
        )?;
    }
    Ok(())
}

/// Renders a score: prints each stem's levels, writes the stems if asked,
/// and the stereo mix. Patch files are found from `dir`.
fn render(
    score: &Score,
    dir: &Path,
    mix: &Path,
    stems: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let audio = score.render(dir)?;
    println!(
        "rendered {} stems of {:.1} s in {:.1} s",
        audio.len(),
        score.seconds(),
        started.elapsed().as_secs_f64()
    );
    println!("{:<14} {:>9} {:>9}", "", "peak", "rms");
    for (stem, samples) in score.stems.iter().zip(&audio) {
        println!(
            "{:<14} {:>9} {:>9}",
            stem.name,
            db(peak(samples)),
            db(rms(samples))
        );
    }
    if let Some(folder) = stems {
        fs::create_dir_all(folder)?;
        for (stem, samples) in score.stems.iter().zip(&audio) {
            wav::write(
                &folder.join(file_name(&stem.name)),
                score.sample_rate,
                1,
                samples,
            )?;
        }
    }
    let mixed = score.mix(&audio);
    println!(
        "{:<14} {:>9} {:>9}",
        "mix",
        db(peak(&mixed)),
        db(rms(&mixed))
    );
    if peak(&mixed) > 1.0 {
        println!("the mix goes above 0 dB: nothing is clipped in the float file, master it");
    }
    wav::write(mix, score.sample_rate, 2, &mixed)?;
    println!("wrote {}", mix.display());
    Ok(())
}

fn master(mix: &Path, wav: &Path, mp3: &Path, lufs: f64) -> Result<(), Box<dyn Error>> {
    let (before, after) = media::master(mix, wav, mp3, lufs)?;
    println!(
        "mix     {:6.1} LUFS  true peak {:5.1} dBTP",
        before.integrated, before.true_peak
    );
    println!(
        "master  {:6.1} LUFS  true peak {:5.1} dBTP  range {:.1} LU",
        after.integrated, after.true_peak, after.range
    );
    println!("wrote {}\nwrote {}", wav.display(), mp3.display());
    Ok(())
}

/// Renders a patch file or a built-in preset on its own, as a mono WAV.
fn sound(patch: &str, seconds: f64, output: Option<String>) -> Result<(), Box<dyn Error>> {
    let path = Path::new(patch);
    let (name, reference) = if path.is_file() {
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        (
            name,
            std::path::absolute(path)?.to_string_lossy().into_owned(),
        )
    } else if rack::preset(patch).is_some() {
        (patch.to_string(), patch.to_string())
    } else {
        let presets: Vec<_> = rack::preset_names().collect();
        return Err(format!(
            "no patch file or preset named {patch:?} (presets: {})",
            presets.join(", ")
        )
        .into());
    };
    let score = Score::single(&reference, seconds);
    let audio = score.render(Path::new(""))?.remove(0);
    let out = output.map_or_else(
        || renders().join("sounds").join(format!("{name}.wav")),
        PathBuf::from,
    );
    if let Some(folder) = out.parent() {
        fs::create_dir_all(folder)?;
    }
    wav::write(&out, score.sample_rate, 1, &audio)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn peak(audio: &[f32]) -> f32 {
    audio.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn rms(audio: &[f32]) -> f32 {
    (audio.iter().map(|s| s * s).sum::<f32>() / audio.len().max(1) as f32).sqrt()
}

fn db(level: f32) -> String {
    if level > 0.0 {
        format!("{:.1} dB", 20.0 * level.log10())
    } else {
        "-inf dB".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        Args(list.iter().map(|a| a.to_string()).collect())
    }

    #[test]
    fn options_come_out_by_name_in_any_order() {
        let mut a = args(&["--bars", "8", "demo", "--no-master"]);
        assert_eq!(a.number("--bars"), Ok(Some(8.0)));
        assert!(a.flag("--no-master"));
        assert!(!a.flag("--no-master"));
        assert_eq!(a.one(), Ok("demo".to_string()));
    }

    #[test]
    fn mistakes_are_named() {
        assert!(
            args(&["--lufs"])
                .value("--lufs")
                .unwrap_err()
                .contains("needs a value")
        );
        assert!(
            args(&["--zoom", "x"])
                .number("--zoom")
                .unwrap_err()
                .contains("number")
        );
        assert!(args(&["a", "--what"]).one().unwrap_err().contains("--what"));
        assert!(args(&["a", "b"]).one().unwrap_err().contains("got 2"));
    }

    #[test]
    fn every_track_composes() {
        for (name, compose) in tracks::TRACKS {
            let out = env::temp_dir().join(format!("audiochaos-studio-{name}"));
            let path = compose().write(&out, &tracks_dir().join(name)).unwrap();
            assert!(
                Score::from_json(&fs::read_to_string(path).unwrap()).is_ok(),
                "{name}"
            );
            fs::remove_dir_all(&out).unwrap();
        }
    }

    #[test]
    fn the_template_plays() {
        let out = env::temp_dir().join("audiochaos-studio-template");
        let path = template::compose().write(&out, &out).unwrap();
        let mut score = Score::from_json(&fs::read_to_string(path).unwrap()).unwrap();
        // Up to just after the first snare, on the second beat of bar 8.
        score.length = 34.0;
        let audio = score.render(&out).unwrap();
        fs::remove_dir_all(&out).unwrap();
        assert_eq!(audio.len(), 5);
        let peaks: Vec<f32> = audio.iter().map(|stem| peak(stem)).collect();
        // Every stem plays.
        assert!(
            peaks.iter().all(|&p| p > 0.01),
            "every stem plays: {peaks:?}"
        );
    }
}
