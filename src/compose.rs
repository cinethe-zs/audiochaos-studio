//! Writing tracks: time in beats, pitches, step patterns, and the stems that
//! collect knob moves and cable changes for the renderer.
//!
//! Times are in beats. Durations that shape a sound are in milliseconds and
//! say so in their name (`decay_ms`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use audiochaos::patch::{Patch, PatchCable, VERSION};

use crate::score::Event;

/// A hit's level is set this long before it, while its cable is still
/// unplugged, so the rack's 5 ms knob smoothing has settled when it plugs in.
const PRE_MS: f64 = 20.0;

const NOTE_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];

fn semitone(name: &str) -> Option<i32> {
    Some(match name {
        "C" => 0,
        "C#" | "Db" => 1,
        "D" => 2,
        "D#" | "Eb" => 3,
        "E" => 4,
        "F" => 5,
        "F#" | "Gb" => 6,
        "G" => 7,
        "G#" | "Ab" => 8,
        "A" => 9,
        "A#" | "Bb" => 10,
        "B" => 11,
        _ => return None,
    })
}

/// MIDI number of a note name: "A1", "Bb1", "C#3".
pub fn midi(note: &str) -> i32 {
    let split = note
        .find(|c: char| c == '-' || c.is_ascii_digit())
        .unwrap_or(note.len());
    let (name, octave) = note.split_at(split);
    match (semitone(name), octave.parse::<i32>()) {
        (Some(semitone), Ok(octave)) => 12 * (octave + 1) + semitone,
        _ => panic!("not a note name: {note:?}"),
    }
}

/// Equal-tempered frequency of a note name, with A4 at 440 Hz.
pub fn hz(note: &str) -> f64 {
    440.0 * 2f64.powf(f64::from(midi(note) - 69) / 12.0)
}

pub fn note_name(number: i32) -> String {
    format!(
        "{}{}",
        NOTE_NAMES[number.rem_euclid(12) as usize],
        number.div_euclid(12) - 1
    )
}

pub fn transpose(note: &str, semitones: i32) -> String {
    note_name(midi(note) + semitones)
}

/// The notes of a mode from `root` up, ending on the root `octaves` higher.
/// Modes: major, minor, harmonic minor, dorian, phrygian, lydian,
/// mixolydian, locrian, minor pentatonic, major pentatonic.
pub fn scale(root: &str, mode: &str, octaves: i32) -> Vec<String> {
    let steps: &[i32] = match mode {
        "major" => &[0, 2, 4, 5, 7, 9, 11],
        "minor" => &[0, 2, 3, 5, 7, 8, 10],
        "harmonic minor" => &[0, 2, 3, 5, 7, 8, 11],
        "dorian" => &[0, 2, 3, 5, 7, 9, 10],
        "phrygian" => &[0, 1, 3, 5, 7, 8, 10],
        "lydian" => &[0, 2, 4, 6, 7, 9, 11],
        "mixolydian" => &[0, 2, 4, 5, 7, 9, 10],
        "locrian" => &[0, 1, 3, 5, 6, 8, 10],
        "minor pentatonic" => &[0, 3, 5, 7, 10],
        "major pentatonic" => &[0, 2, 4, 7, 9],
        _ => panic!("unknown mode {mode:?}"),
    };
    let base = midi(root);
    let mut notes: Vec<String> = (0..octaves)
        .flat_map(|octave| {
            steps
                .iter()
                .map(move |step| note_name(base + 12 * octave + step))
        })
        .collect();
    notes.push(note_name(base + 12 * octaves));
    notes
}

/// A step pattern spreading `pulses` hits as evenly as possible over
/// `steps`: euclid(3, 8, 0) is "x..x..x.".
pub fn euclid(pulses: usize, steps: usize, rotate: usize) -> String {
    let pattern: Vec<char> = (0..steps)
        .map(|i| {
            if (i * pulses) % steps < pulses {
                'x'
            } else {
                '.'
            }
        })
        .collect();
    let rotate = rotate % steps;
    pattern[rotate..].iter().chain(&pattern[..rotate]).collect()
}

/// (time, level) hits with their levels scaled from `start` to `end` across
/// them: a crescendo, or a decrescendo.
pub fn fade(hits: &[(f64, f64)], start: f64, end: f64) -> Vec<(f64, f64)> {
    let (t0, t1) = (hits[0].0, hits[hits.len() - 1].0);
    let span = if t1 - t0 != 0.0 { t1 - t0 } else { 1.0 };
    hits.iter()
        .map(|&(t, level)| (t, level * (start + (end - start) * (t - t0) / span)))
        .collect()
}

/// A pitch: a note name, or a frequency in hertz.
pub trait Pitch {
    fn hz(&self) -> f64;
}

impl Pitch for f64 {
    fn hz(&self) -> f64 {
        *self
    }
}

impl Pitch for i32 {
    fn hz(&self) -> f64 {
        f64::from(*self)
    }
}

impl Pitch for &str {
    fn hz(&self) -> f64 {
        hz(self)
    }
}

impl Pitch for String {
    fn hz(&self) -> f64 {
        hz(self)
    }
}

/// A note of a melody.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub at: f64,
    pub pitch: String,
    pub length: f64,
    pub accent: bool,
}

/// Tempo and meter: what turns bars and milliseconds into beats.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub bpm: f64,
    pub beats_per_bar: f64,
}

impl Clock {
    /// When bar `n` starts, bars counted from 0.
    pub fn bar(&self, n: impl Into<f64>) -> f64 {
        self.beats_per_bar * n.into()
    }

    /// A beat (from 0) within a bar (from 0).
    pub fn at(&self, bar: impl Into<f64>, beat: impl Into<f64>) -> f64 {
        self.beats_per_bar * bar.into() + beat.into()
    }

    /// A duration in milliseconds, in beats.
    pub fn ms(&self, milliseconds: impl Into<f64>) -> f64 {
        milliseconds.into() / 1000.0 * self.bpm / 60.0
    }
}

/// A piece: a tempo, a length in bars, section markers, and its stems.
pub struct Track {
    pub clock: Clock,
    pub bars: f64,
    pub sample_rate: u32,
    markers: Vec<(String, f64)>,
    stems: Vec<Stem>,
}

impl Track {
    /// A track in 4/4.
    pub fn new(bpm: impl Into<f64>, bars: impl Into<f64>) -> Track {
        Track {
            clock: Clock {
                bpm: bpm.into(),
                beats_per_bar: 4.0,
            },
            bars: bars.into(),
            sample_rate: 48_000,
            markers: Vec::new(),
            stems: Vec::new(),
        }
    }

    pub fn meter(mut self, beats_per_bar: impl Into<f64>) -> Track {
        self.clock.beats_per_bar = beats_per_bar.into();
        self
    }

    pub fn bar(&self, n: impl Into<f64>) -> f64 {
        self.clock.bar(n)
    }

    pub fn at(&self, bar: impl Into<f64>, beat: impl Into<f64>) -> f64 {
        self.clock.at(bar, beat)
    }

    pub fn ms(&self, milliseconds: impl Into<f64>) -> f64 {
        self.clock.ms(milliseconds)
    }

    /// Names the section starting at `bar`, for the analysis.
    pub fn mark(&mut self, name: &str, bar: impl Into<f64>) {
        let at = self.bar(bar);
        self.markers.push((name.to_string(), at));
    }

    /// Adds a stem, an instrument or a stereo pair, after the ones already
    /// there.
    pub fn add(&mut self, part: impl Part) {
        self.stems.extend(part.into_stems());
    }

    pub fn stems(&self) -> &[Stem] {
        &self.stems
    }

    /// Every step from bar `first` to bar `last`; swing delays every other
    /// step by that fraction of a step.
    fn grid(&self, first: f64, last: f64, step: f64, swing: f64) -> Vec<f64> {
        let count = ((last - first) * self.clock.beats_per_bar / step).round_ties_even() as usize;
        let start = self.bar(first);
        (0..count)
            .map(|i| {
                let t = start + i as f64 * step;
                if swing != 0.0 && i % 2 == 1 {
                    t + swing * step
                } else {
                    t
                }
            })
            .collect()
    }

    /// (time, level) hits of a pattern repeated from bar `first` up to bar
    /// `last`, one character per sixteenth: "." a rest, "x" a full hit, "1"
    /// to "9" softer ones. Spaces are ignored. A pattern that does not fill
    /// whole bars runs across them.
    pub fn steps(
        &self,
        spec: &str,
        first: impl Into<f64>,
        last: impl Into<f64>,
    ) -> Vec<(f64, f64)> {
        self.steps_with(spec, first, last, 0.25, 0.0)
    }

    /// `steps` with another step length (in beats) and swing.
    pub fn steps_with(
        &self,
        spec: &str,
        first: impl Into<f64>,
        last: impl Into<f64>,
        step: f64,
        swing: f64,
    ) -> Vec<(f64, f64)> {
        let symbols = symbols(spec);
        self.grid(first.into(), last.into(), step, swing)
            .into_iter()
            .enumerate()
            .filter_map(|(i, t)| match symbols[i % symbols.len()] {
                '.' => None,
                'x' => Some((t, 1.0)),
                c => match c.to_digit(10) {
                    Some(level) => Some((t, f64::from(level) / 9.0)),
                    None => panic!("unknown step {c:?} in {spec:?}"),
                },
            })
            .collect()
    }

    /// (time, length) notes of a pattern: "x" starts a note, "-" holds it
    /// one more sixteenth, "." is a rest.
    pub fn gates(
        &self,
        spec: &str,
        first: impl Into<f64>,
        last: impl Into<f64>,
    ) -> Vec<(f64, f64)> {
        self.gates_with(spec, first, last, 0.25, 0.0)
    }

    pub fn gates_with(
        &self,
        spec: &str,
        first: impl Into<f64>,
        last: impl Into<f64>,
        step: f64,
        swing: f64,
    ) -> Vec<(f64, f64)> {
        let symbols = symbols(spec);
        let mut notes: Vec<(f64, f64)> = Vec::new();
        let mut holding = false;
        for (i, t) in self
            .grid(first.into(), last.into(), step, swing)
            .into_iter()
            .enumerate()
        {
            match symbols[i % symbols.len()] {
                'x' => {
                    notes.push((t, step));
                    holding = true;
                }
                '-' if holding => notes.last_mut().expect("holding a note").1 += step,
                _ => holding = false,
            }
        }
        notes
    }

    /// Notes of a line, one space-separated token per sixteenth: a note name
    /// ("A1", or "Bb2!" with an accent), "." a rest, "-" holding the
    /// previous note one more step. "|" bar lines are ignored.
    pub fn melody(&self, spec: &str, first: impl Into<f64>, last: impl Into<f64>) -> Vec<Note> {
        self.melody_with(spec, first, last, 0.25, 0.0)
    }

    pub fn melody_with(
        &self,
        spec: &str,
        first: impl Into<f64>,
        last: impl Into<f64>,
        step: f64,
        swing: f64,
    ) -> Vec<Note> {
        let tokens: Vec<&str> = spec.split_whitespace().filter(|t| *t != "|").collect();
        assert!(!tokens.is_empty(), "empty melody");
        let mut notes: Vec<Note> = Vec::new();
        let mut holding = false;
        for (i, t) in self
            .grid(first.into(), last.into(), step, swing)
            .into_iter()
            .enumerate()
        {
            match tokens[i % tokens.len()] {
                "-" => {
                    if holding {
                        notes.last_mut().expect("holding a note").length += step;
                    }
                }
                "." => holding = false,
                token => {
                    let pitch = token.trim_end_matches('!');
                    midi(pitch);
                    notes.push(Note {
                        at: t,
                        pitch: pitch.to_string(),
                        length: step,
                        accent: token.ends_with('!'),
                    });
                    holding = true;
                }
            }
        }
        notes
    }

    /// Writes the score to `out/track.json`, one event per line, and the
    /// stems' patches to `out/patches/`. Patch files given to `Stem::file`
    /// are read from `sources`, the track's folder.
    pub fn write(&self, out: &Path, sources: &Path) -> Result<PathBuf, String> {
        let mut names: Vec<&str> = self.stems.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(format!("two stems are named {:?}", pair[0]));
        }
        let patches = out.join("patches");
        fs::create_dir_all(&patches)
            .map_err(|e| format!("cannot create {}: {e}", patches.display()))?;
        for entry in fs::read_dir(&patches).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "json") {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
        }
        let mut stems = Vec::new();
        for stem in &self.stems {
            let reference = match &stem.source {
                Source::Preset(preset) => preset.clone(),
                Source::Patch(patch) => write_patch(&patches, &stem.name, patch)?,
                Source::File(file) => {
                    let path = sources.join(file);
                    let json = fs::read_to_string(&path)
                        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                    let patch =
                        Patch::from_json(&json).map_err(|e| format!("{}: {e}", path.display()))?;
                    write_patch(&patches, &stem.name, &patch)?
                }
            };
            let mut events: Vec<&Event> = stem.events.iter().collect();
            events.sort_by(|a, b| a.at.total_cmp(&b.at));
            let lines: Vec<String> = events
                .iter()
                .map(|e| format!("      {}", json(e)))
                .collect();
            stems.push(format!(
                "{{\"name\": {}, \"patch\": {}, \"gain_db\": {}, \"pan\": {}, \"events\": [\n{}\n    ]}}",
                json(&stem.name),
                json(&reference),
                json(&stem.gain_db),
                json(&stem.pan),
                lines.join(",\n")
            ));
        }
        let markers: serde_json::Map<String, serde_json::Value> = self
            .markers
            .iter()
            .map(|(name, at)| (name.clone(), (*at).into()))
            .collect();
        let meta =
            serde_json::json!({ "beats_per_bar": self.clock.beats_per_bar, "markers": markers });
        let sample_rate = match self.sample_rate {
            48_000 => String::new(),
            rate => format!("\"sample_rate\": {rate}, "),
        };
        let text = format!(
            "{{\"bpm\": {}, \"length\": {}, {sample_rate}\"meta\": {meta}, \"stems\": [\n    {}\n]}}\n",
            json(&self.clock.bpm),
            json(&(self.bars * self.clock.beats_per_bar)),
            stems.join(",\n    ")
        );
        serde_json::from_str::<serde_json::Value>(&text).map_err(|e| format!("bad score: {e}"))?;
        let path = out.join("track.json");
        fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path)
    }
}

fn symbols(spec: &str) -> Vec<char> {
    let symbols: Vec<char> = spec.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(!symbols.is_empty(), "empty pattern");
    symbols
}

fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).expect("plain values serialize")
}

fn write_patch(folder: &Path, name: &str, patch: &Patch) -> Result<String, String> {
    let path = folder.join(format!("{name}.json"));
    fs::write(&path, patch.to_json() + "\n")
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(format!("patches/{name}.json"))
}

/// Rounds to 4 decimals, as the score is written: under a tenth of a
/// millisecond at any usual tempo.
fn round4(x: f64) -> f64 {
    format!("{x:.4}")
        .parse()
        .expect("a formatted number parses")
}

/// Splits "module/output -> module/input".
fn cable_ends(spec: &str) -> (String, String, String, String) {
    let parts: Option<Vec<(&str, &str)>> = spec
        .split("->")
        .map(|side| {
            side.trim()
                .split_once('/')
                .map(|(m, p)| (m.trim(), p.trim()))
        })
        .collect();
    match parts.as_deref() {
        Some([(from, output), (to, input)]) => (
            from.to_string(),
            output.to_string(),
            to.to_string(),
            input.to_string(),
        ),
        _ => panic!("{spec:?} is not \"module/output -> module/input\""),
    }
}

/// Builds a patch for the stock rack, in the audiochaos format. Knobs left
/// out keep their defaults.
pub struct Patcher(Patch);

impl Patcher {
    pub fn new(description: &str) -> Patcher {
        Patcher(Patch {
            version: VERSION,
            description: description.to_string(),
            knobs: BTreeMap::new(),
            cables: Vec::new(),
            positions: BTreeMap::new(),
        })
    }

    pub fn knob(mut self, module: &str, knob: &str, value: f64) -> Patcher {
        self.0
            .knobs
            .entry(module.to_string())
            .or_default()
            .insert(knob.to_string(), value as f32);
        self
    }

    /// Adds a cable, "module/output -> module/input".
    pub fn cable(mut self, spec: &str) -> Patcher {
        let (from, output, to, input) = cable_ends(spec);
        self.0.cables.push(PatchCable {
            from,
            output,
            to,
            input,
        });
        self
    }

    pub fn build(self) -> Patch {
        self.0
    }
}

/// Where a stem's patch comes from.
pub enum Source {
    Patch(Patch),
    Preset(String),
    /// A patch file, relative to the track's folder.
    File(PathBuf),
}

/// Playing a stem by hand: knob moves and cable changes, at times in beats.
pub trait Voice {
    /// Turns a knob ("module/knob") to `value` at once. The rack still
    /// glides there in 5 ms: a sharp attack needs a cable.
    fn set(&mut self, at: f64, knob: &str, value: f64);

    /// Turns a knob to `value` over `over` beats. `curve` shapes the move:
    /// 0 is straight, positive moves fast then settles (decays), negative
    /// starts slow (risers).
    fn glide(&mut self, at: f64, knob: &str, value: f64, over: f64, curve: f64);

    /// Plugs a cable, "module/output -> module/input".
    fn connect(&mut self, at: f64, cable: &str);

    fn disconnect(&mut self, at: f64, cable: &str);
}

/// What a track holds: a stem, an instrument, a stereo pair.
pub trait Part {
    /// The finished stems, with the events held back until all the notes
    /// were known.
    fn into_stems(self) -> Vec<Stem>;
}

/// One rack of the track: its patch and the events that play it.
pub struct Stem {
    pub name: String,
    pub source: Source,
    pub gain_db: f64,
    pub pan: f64,
    clock: Clock,
    events: Vec<Event>,
}

impl Stem {
    pub fn new(track: &Track, name: &str, patch: Patch) -> Stem {
        Stem::from_source(track, name, Source::Patch(patch))
    }

    /// A built-in preset of audiochaos, such as "chaos" or "bass".
    pub fn preset(track: &Track, name: &str, preset: &str) -> Stem {
        Stem::from_source(track, name, Source::Preset(preset.to_string()))
    }

    /// A patch file saved from the audiochaos editor, relative to the
    /// track's folder.
    pub fn file(track: &Track, name: &str, path: impl Into<PathBuf>) -> Stem {
        Stem::from_source(track, name, Source::File(path.into()))
    }

    fn from_source(track: &Track, name: &str, source: Source) -> Stem {
        Stem {
            name: name.to_string(),
            source,
            gain_db: 0.0,
            pan: 0.0,
            clock: track.clock,
            events: Vec::new(),
        }
    }

    /// Level in dB and place from -1 (left) to 1 (right) in the mix.
    pub fn mix(mut self, gain_db: f64, pan: f64) -> Stem {
        self.gain_db = gain_db;
        self.pan = pan;
        self
    }

    pub fn clock(&self) -> Clock {
        self.clock
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// When to prepare a hit at `at`: just before it, in silence.
    pub fn ahead(&self, at: f64) -> f64 {
        (at - self.clock.ms(PRE_MS)).max(0.0)
    }

    /// Percussive hits, (time, level, decay in beats). Plugging `cable` is
    /// the attack, instant where a knob would glide; then `gain` falls to 0
    /// (sooner when the next hit comes first), the cable is pulled, and the
    /// next level is set in silence.
    pub fn strike(&mut self, hits: &[(f64, f64, f64)], cable: &str, gain: &str, curve: f64) {
        let clock = self.clock;
        let ms = |milliseconds: f64| clock.ms(milliseconds);
        let mut hits = hits.to_vec();
        hits.sort_by(|a, b| a.partial_cmp(b).expect("hits are numbers"));
        for (i, &(t, level, decay)) in hits.iter().enumerate() {
            let following = hits.get(i + 1).map_or(f64::INFINITY, |next| next.0);
            let length = decay.min(following - t - ms(PRE_MS) - ms(17.0));
            assert!(
                length > ms(5.0),
                "{}: hits too close at beat {t}",
                self.name
            );
            self.set(self.ahead(t), gain, level);
            self.connect(t, cable);
            self.glide(t, gain, 0.0, length, curve);
            // The gain glides behind its ramp: pull the cable as late as the
            // next hit allows, so that nothing is left to cut.
            let pull = (t + length + ms(60.0)).min(following - ms(PRE_MS) - ms(2.0));
            self.disconnect(pull, cable);
        }
    }
}

impl Voice for Stem {
    fn set(&mut self, at: f64, knob: &str, value: f64) {
        self.events.push(Event {
            at: round4(at),
            knob: Some(knob.to_string()),
            value: Some(round4(value)),
            ..Event::default()
        });
    }

    fn glide(&mut self, at: f64, knob: &str, value: f64, over: f64, curve: f64) {
        self.events.push(Event {
            at: round4(at),
            knob: Some(knob.to_string()),
            value: Some(round4(value)),
            ramp: round4(over),
            curve,
            ..Event::default()
        });
    }

    fn connect(&mut self, at: f64, cable: &str) {
        self.events.push(Event {
            at: round4(at),
            connect: Some(cable.to_string()),
            ..Event::default()
        });
    }

    fn disconnect(&mut self, at: f64, cable: &str) {
        self.events.push(Event {
            at: round4(at),
            disconnect: Some(cable.to_string()),
            ..Event::default()
        });
    }
}

impl Part for Stem {
    fn into_stems(self) -> Vec<Stem> {
        vec![self]
    }
}

/// Two stems played as one: what is played on the pair goes to the left
/// one, then the right one. Make them differ slightly (a few hertz, a few
/// milliseconds) for a wide sound.
pub struct Stereo<T> {
    pub left: T,
    pub right: T,
}

impl<T> Stereo<T> {
    pub fn new(left: T, right: T) -> Stereo<T> {
        Stereo { left, right }
    }

    /// Plays the same thing on both sides, left first.
    pub fn both(&mut self, mut play: impl FnMut(&mut T)) {
        play(&mut self.left);
        play(&mut self.right);
    }
}

impl<T: Voice> Voice for Stereo<T> {
    fn set(&mut self, at: f64, knob: &str, value: f64) {
        self.both(|side| side.set(at, knob, value));
    }

    fn glide(&mut self, at: f64, knob: &str, value: f64, over: f64, curve: f64) {
        self.both(|side| side.glide(at, knob, value, over, curve));
    }

    fn connect(&mut self, at: f64, cable: &str) {
        self.both(|side| side.connect(at, cable));
    }

    fn disconnect(&mut self, at: f64, cable: &str) {
        self.both(|side| side.disconnect(at, cable));
    }
}

impl<T: Part> Part for Stereo<T> {
    fn into_stems(self) -> Vec<Stem> {
        let mut stems = self.left.into_stems();
        stems.extend(self.right.into_stems());
        stems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names() {
        assert_eq!(hz("A4"), 440.0);
        assert!((hz("A1") - 55.0).abs() < 1e-9);
        assert_eq!(midi("C#3"), midi("Db3"));
        assert_eq!(transpose("A1", 13), "Bb2");
        assert_eq!(note_name(midi("C-1")), "C-1");
    }

    #[test]
    #[should_panic(expected = "not a note name")]
    fn unknown_notes_are_refused() {
        midi("H2");
    }

    #[test]
    fn scales() {
        assert_eq!(
            scale("A2", "phrygian", 1),
            ["A2", "Bb2", "C3", "D3", "E3", "F3", "G3", "A3"]
        );
        assert_eq!(scale("C3", "minor pentatonic", 2).len(), 11);
    }

    #[test]
    fn steps_repeat_over_the_bars() {
        let t = Track::new(120, 4);
        let hits = t.steps("x.5.", 1, 2);
        assert_eq!(hits[..3], [(4.0, 1.0), (4.5, 5.0 / 9.0), (5.0, 1.0)]);
        assert_eq!(hits.len(), 8);
        assert_eq!(
            t.steps_with("xx", 0, 1, 0.25, 0.5)[..2],
            [(0.0, 1.0), (0.375, 1.0)]
        );
    }

    #[test]
    fn gates_are_held_by_dashes() {
        let t = Track::new(120, 4);
        assert_eq!(
            t.gates_with("x-.x x---", 0, 1, 0.5, 0.0),
            [(0.0, 1.0), (1.5, 0.5), (2.0, 2.0)]
        );
    }

    #[test]
    fn melodies_hold_rest_and_accent() {
        let t = Track::new(120, 4);
        let notes = t.melody("A1 - . C2!", 0, 1);
        let note = |at, pitch: &str, length, accent| Note {
            at,
            pitch: pitch.into(),
            length,
            accent,
        };
        assert_eq!(
            notes[..2],
            [note(0.0, "A1", 0.5, false), note(0.75, "C2", 0.25, true)]
        );
    }

    #[test]
    fn euclidean_rhythms_and_fades() {
        assert_eq!(euclid(3, 8, 0), "x..x..x.");
        assert_eq!(euclid(3, 8, 1), "..x..x.x");
        assert_eq!(
            fade(&[(0.0, 1.0), (1.0, 1.0), (2.0, 0.5)], 0.0, 1.0),
            [(0.0, 0.0), (1.0, 0.5), (2.0, 0.5)]
        );
    }

    #[test]
    fn hits_are_cut_short_before_the_next_one() {
        // A beat lasts a second: ms(x) is x / 1000.
        let t = Track::new(60, 1);
        let mut stem = Stem::preset(&t, "hats", "bass");
        stem.strike(
            &[(0.3, 1.0, 0.4), (0.1, 1.0, 0.4)],
            "noise/white -> vca/in",
            "vca/gain",
            3.0,
        );
        let falls: Vec<f64> = stem
            .events()
            .iter()
            .filter(|e| e.ramp > 0.0)
            .map(|e| e.ramp)
            .collect();
        assert_eq!(falls, [round4(0.3 - 0.1 - 0.02 - 0.017), 0.4]);
        let pulls: Vec<f64> = stem
            .events()
            .iter()
            .filter(|e| e.disconnect.is_some())
            .map(|e| e.at)
            .collect();
        assert!(
            pulls[0] < 0.3 - 0.02,
            "pulled before the next hit is prepared"
        );
        assert!(
            (pulls[1] - 0.76).abs() < 1e-9,
            "pulled long after the last one"
        );
    }

    #[test]
    #[should_panic(expected = "hits too close")]
    fn hits_on_top_of_each_other_are_refused() {
        let t = Track::new(60, 1);
        let mut stem = Stem::preset(&t, "hats", "bass");
        stem.strike(
            &[(0.1, 1.0, 0.4), (0.13, 1.0, 0.4)],
            "noise/white -> vca/in",
            "vca/gain",
            3.0,
        );
    }

    #[test]
    fn stereo_plays_both_sides() {
        let t = Track::new(120, 1);
        let mut pair = Stereo::new(
            Stem::preset(&t, "left", "chaos"),
            Stem::preset(&t, "right", "chaos"),
        );
        pair.glide(1.0, "vca/gain", 0.5, 2.0, -1.0);
        assert_eq!(pair.left.events().len(), 1);
        assert_eq!(pair.left.events(), pair.right.events());
    }

    #[test]
    fn writes_the_score_and_the_patches() {
        let mut t = Track::new(120, 2);
        t.mark("start", 0);
        let patch = Patcher::new("a saw")
            .knob("vca", "gain", 0.5)
            .cable("osc 1/saw -> vca/in")
            .build();
        let mut saw = Stem::new(&t, "saw", patch);
        saw.set(1.0, "vca/gain", 0.0);
        t.add(saw);
        t.add(Stem::preset(&t, "preset", "chaos").mix(-3.0, 0.5));
        let out = std::env::temp_dir().join("audiochaos-studio-write-test");
        let path = t.write(&out, &out).unwrap();
        let score: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let patch =
            Patch::from_json(&fs::read_to_string(out.join("patches/saw.json")).unwrap()).unwrap();
        fs::remove_dir_all(&out).unwrap();
        assert_eq!(score["length"], 8.0);
        assert_eq!(score["meta"]["markers"]["start"], 0.0);
        assert_eq!(score["stems"][0]["patch"], "patches/saw.json");
        assert_eq!(score["stems"][1]["patch"], "chaos");
        assert_eq!(score["stems"][1]["gain_db"], -3.0);
        assert_eq!(score["stems"][0]["events"][0]["value"], 0.0);
        assert_eq!(patch.cables[0].input, "in");
    }

    #[test]
    fn stem_names_are_unique() {
        let mut t = Track::new(120, 1);
        t.add(Stem::preset(&t, "same", "chaos"));
        t.add(Stem::preset(&t, "same", "bass"));
        assert!(
            t.write(Path::new("."), Path::new("."))
                .unwrap_err()
                .contains("same")
        );
    }
}
