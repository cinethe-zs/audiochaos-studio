//! Ready-made instruments: each one builds an audiochaos patch and knows the
//! moves that play it. Their default sounds make a dark, industrial kit; the
//! knob ranges are those of the audiochaos editor.
//!
//! Drums collect their hits and place them when added to the track, so that
//! each hit can be cut short before the next one.

use audiochaos::modules::{filter, sequencer};

use crate::compose::{Note, Part, Patcher, Pitch, Stem, Track, Voice};

/// A drum hit. Its decay and pitch can differ from the instrument's.
#[derive(Clone, Debug)]
pub struct Hit {
    pub at: f64,
    pub level: f64,
    pub decay_ms: Option<f64>,
    pub tune: Option<f64>,
}

impl Hit {
    pub fn decay_ms(&mut self, milliseconds: f64) -> &mut Hit {
        self.decay_ms = Some(milliseconds);
        self
    }

    /// The pitch a kick falls to, in hertz.
    pub fn tune(&mut self, hz: f64) -> &mut Hit {
        self.tune = Some(hz);
        self
    }
}

/// Knob moves and cable changes go to the instrument's stem, and so does its
/// place in the mix.
macro_rules! instrument {
    ($($kind:ident),*) => {$(
        impl Voice for $kind {
            fn set(&mut self, at: f64, knob: &str, value: f64) {
                self.stem.set(at, knob, value);
            }
            fn glide(&mut self, at: f64, knob: &str, value: f64, over: f64, curve: f64) {
                self.stem.glide(at, knob, value, over, curve);
            }
            fn connect(&mut self, at: f64, cable: &str) {
                self.stem.connect(at, cable);
            }
            fn disconnect(&mut self, at: f64, cable: &str) {
                self.stem.disconnect(at, cable);
            }
        }

        impl $kind {
            /// Level in dB and place from -1 (left) to 1 (right) in the mix.
            pub fn mix(mut self, gain_db: f64, pan: f64) -> $kind {
                self.stem = self.stem.mix(gain_db, pan);
                self
            }
        }
    )*};
}

macro_rules! drum {
    ($($kind:ident),*) => {$(
        impl $kind {
            /// Adds a hit; what it returns can change its decay or pitch.
            pub fn hit(&mut self, at: f64, level: f64) -> &mut Hit {
                self.hits.push(Hit { at, level, decay_ms: None, tune: None });
                self.hits.last_mut().expect("a hit was just added")
            }

            /// Adds (time, level) hits, as `Track::steps` makes them.
            pub fn hits(&mut self, hits: impl IntoIterator<Item = (f64, f64)>) {
                for (at, level) in hits {
                    self.hit(at, level);
                }
            }

            /// When it hits, in the order the hits were added: to duck other
            /// stems under it.
            pub fn times(&self) -> Vec<f64> {
                self.hits.iter().map(|hit| hit.at).collect()
            }
        }
    )*};
}

instrument!(
    Kick, Snare, Hat, Metal, Mono, Chaos, Drone, Fx, Hum, Pump, Space
);
drum!(Kick, Snare, Hat, Metal, Space);

/// How a Kick sounds.
#[derive(Clone, Debug)]
pub struct KickSound {
    /// The pitch it falls to, in hertz.
    pub tune: f64,
    /// The pitch it starts from, in hertz.
    pub click: f64,
    pub sweep_ms: f64,
    pub decay_ms: f64,
    pub drive: f64,
    /// A low-pass after the distortion, at this cutoff in hertz, if any: the
    /// score can then muffle the kick and open it again ("filter/cutoff").
    pub lowpass: Option<f64>,
}

impl Default for KickSound {
    fn default() -> KickSound {
        KickSound {
            tune: 55.0,
            click: 900.0,
            sweep_ms: 90.0,
            decay_ms: 330.0,
            drive: 2.5,
            lowpass: None,
        }
    }
}

/// Sine kick: each hit plugs osc 1 into the VCA while its pitch falls from
/// `click` to `tune` and the VCA closes. The distortion comes after the VCA,
/// so the head of the kick is driven harder than its tail. An optional
/// low-pass follows it.
pub struct Kick {
    stem: Stem,
    sound: KickSound,
    hits: Vec<Hit>,
}

impl Kick {
    pub fn new(track: &Track, name: &str) -> Kick {
        Kick::with(track, name, KickSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: KickSound) -> Kick {
        let lowpass = if sound.lowpass.is_some() {
            " A low-pass after it can muffle the kick."
        } else {
            ""
        };
        let patch = Patcher::new(&format!(
            "Sine kick. Each hit plugs osc 1 into the VCA, then the score drops the pitch from \
             {} Hz to {} Hz and closes the VCA. The distortion comes after the VCA, so the head \
             of the kick is driven harder than its tail.{lowpass}",
            sound.click, sound.tune
        ))
        .knob("osc 1", "freq", sound.tune)
        .knob("vca", "gain", 0.0)
        .knob("dist", "drive", sound.drive)
        .knob("dist", "fold", 0.0)
        .knob("master", "volume", 1.0)
        .cable("vca/out -> dist/in");
        let patch = match sound.lowpass {
            Some(cutoff) => patch
                .knob("filter", "cutoff", cutoff)
                .knob("filter", "resonance", 0.3)
                .cable("dist/out -> filter/in")
                .cable("filter/lowpass -> master/in"),
            None => patch.cable("dist/out -> master/in"),
        }
        .build();
        Kick {
            stem: Stem::new(track, name, patch),
            sound,
            hits: Vec::new(),
        }
    }
}

impl Part for Kick {
    fn into_stems(mut self) -> Vec<Stem> {
        let clock = self.stem.clock();
        for hit in &self.hits {
            let ahead = self.stem.ahead(hit.at);
            self.stem.set(ahead, "osc 1/freq", self.sound.click);
            let tune = hit.tune.unwrap_or(self.sound.tune);
            self.stem.glide(
                hit.at,
                "osc 1/freq",
                tune,
                clock.ms(self.sound.sweep_ms),
                6.0,
            );
        }
        let strikes: Vec<_> = self
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.at,
                    hit.level,
                    clock.ms(hit.decay_ms.unwrap_or(self.sound.decay_ms)),
                )
            })
            .collect();
        self.stem
            .strike(&strikes, "osc 1/sine -> vca/in", "vca/gain", 2.0);
        vec![self.stem]
    }
}

/// How a Snare sounds.
#[derive(Clone, Debug)]
pub struct SnareSound {
    /// Centre of the band of noise, in hertz.
    pub tone: f64,
    pub resonance: f64,
    /// The pitch the body falls to, from `body_click`, in hertz.
    pub body: f64,
    pub body_click: f64,
    pub body_level: f64,
    pub drive: f64,
    pub fold: f64,
    /// The metal room: a short delay with feedback.
    pub room_ms: f64,
    pub room_feedback: f64,
    pub room_mix: f64,
    pub decay_ms: f64,
}

impl Default for SnareSound {
    fn default() -> SnareSound {
        SnareSound {
            tone: 1800.0,
            resonance: 0.45,
            body: 185.0,
            body_click: 330.0,
            body_level: 0.55,
            drive: 3.5,
            fold: 0.25,
            room_ms: 21.0,
            room_feedback: 0.55,
            room_mix: 0.35,
            decay_ms: 160.0,
        }
    }
}

/// Industrial snare: band-passed noise and a sine body through the
/// distortion. Each hit plugs the distortion into the VCA, and the short
/// delay after it rings like a metal room.
pub struct Snare {
    stem: Stem,
    sound: SnareSound,
    hits: Vec<Hit>,
}

impl Snare {
    pub fn new(track: &Track, name: &str) -> Snare {
        Snare::with(track, name, SnareSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: SnareSound) -> Snare {
        let patch = Patcher::new(
            "Industrial snare: band-passed noise and a sine body through the distortion. Each \
             hit plugs the distortion into the VCA, and the short delay after it rings like a \
             metal room.",
        )
        .knob("filter", "cutoff", sound.tone)
        .knob("filter", "resonance", sound.resonance)
        .knob("osc 2", "freq", sound.body)
        .knob("atten", "gain a", sound.body_level)
        .knob("dist", "drive", sound.drive)
        .knob("dist", "fold", sound.fold)
        .knob("delay", "time", sound.room_ms / 1000.0)
        .knob("delay", "feedback", sound.room_feedback)
        .knob("delay", "mix", sound.room_mix)
        .knob("vca", "gain", 0.0)
        .knob("master", "volume", 1.0)
        .cable("noise/white -> filter/in")
        .cable("filter/bandpass -> dist/in")
        .cable("osc 2/sine -> atten/a")
        .cable("atten/a -> dist/in")
        .cable("vca/out -> delay/in")
        .cable("delay/out -> master/in")
        .build();
        Snare {
            stem: Stem::new(track, name, patch),
            sound,
            hits: Vec::new(),
        }
    }
}

impl Part for Snare {
    fn into_stems(mut self) -> Vec<Stem> {
        let clock = self.stem.clock();
        for hit in &self.hits {
            let ahead = self.stem.ahead(hit.at);
            self.stem.set(ahead, "osc 2/freq", self.sound.body_click);
            self.stem
                .glide(hit.at, "osc 2/freq", self.sound.body, clock.ms(40.0), 4.0);
        }
        let strikes: Vec<_> = self
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.at,
                    hit.level,
                    clock.ms(hit.decay_ms.unwrap_or(self.sound.decay_ms)),
                )
            })
            .collect();
        self.stem
            .strike(&strikes, "dist/out -> vca/in", "vca/gain", 3.0);
        vec![self.stem]
    }
}

/// How a Hat sounds.
#[derive(Clone, Debug)]
pub struct HatSound {
    pub cutoff: f64,
    pub resonance: f64,
    /// About 40 ms for a closed hat, 170 ms for an open one.
    pub decay_ms: f64,
    pub curve: f64,
    /// Bit depth and downsampling of a bitcrush for grit, if any.
    pub crush: Option<(f64, f64)>,
}

impl Default for HatSound {
    fn default() -> HatSound {
        HatSound {
            cutoff: 7500.0,
            resonance: 0.3,
            decay_ms: 40.0,
            curve: 3.0,
            crush: None,
        }
    }
}

/// Hi-hat: white noise through the high-pass, bitcrushed if asked. Each hit
/// plugs it into the VCA, which closes in `decay_ms`.
pub struct Hat {
    stem: Stem,
    sound: HatSound,
    hits: Vec<Hit>,
    cable: &'static str,
}

impl Hat {
    pub fn new(track: &Track, name: &str) -> Hat {
        Hat::with(track, name, HatSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: HatSound) -> Hat {
        let grit = if sound.crush.is_some() {
            " and a bitcrush for grit"
        } else {
            ""
        };
        let mut patch = Patcher::new(&format!(
            "Hi-hat: white noise through the high-pass{grit}. Each hit plugs it into the VCA, \
             which closes in {} ms.",
            sound.decay_ms
        ))
        .knob("filter", "cutoff", sound.cutoff)
        .knob("filter", "resonance", sound.resonance)
        .knob("vca", "gain", 0.0)
        .knob("master", "volume", 1.0)
        .cable("noise/white -> filter/in");
        let cable = match sound.crush {
            Some((bits, downsample)) => {
                patch = patch
                    .knob("crush", "bits", bits)
                    .knob("crush", "downsample", downsample)
                    .cable("filter/highpass -> crush/in");
                "crush/out -> vca/in"
            }
            None => "filter/highpass -> vca/in",
        };
        Hat {
            stem: Stem::new(track, name, patch.cable("vca/out -> master/in").build()),
            sound,
            hits: Vec::new(),
            cable,
        }
    }
}

impl Part for Hat {
    fn into_stems(mut self) -> Vec<Stem> {
        let clock = self.stem.clock();
        let strikes: Vec<_> = self
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.at,
                    hit.level,
                    clock.ms(hit.decay_ms.unwrap_or(self.sound.decay_ms)),
                )
            })
            .collect();
        self.stem
            .strike(&strikes, self.cable, "vca/gain", self.sound.curve);
        vec![self.stem]
    }
}

/// An oscillator's waveform.
#[derive(Clone, Copy, Debug)]
pub enum Wave {
    Sine,
    Saw,
    Square,
}

impl Wave {
    fn port(self) -> &'static str {
        match self {
            Wave::Sine => "sine",
            Wave::Saw => "saw",
            Wave::Square => "square",
        }
    }
}

/// Which output of the state-variable filter a voice goes through.
#[derive(Clone, Copy, Debug)]
pub enum Filter {
    Lowpass,
    Bandpass,
    Highpass,
}

impl Filter {
    fn port(self) -> &'static str {
        match self {
            Filter::Lowpass => "lowpass",
            Filter::Bandpass => "bandpass",
            Filter::Highpass => "highpass",
        }
    }
}

/// An echo after the VCA: the rack's delay, its time in seconds.
#[derive(Clone, Copy, Debug)]
pub struct Echo {
    pub time: f64,
    pub feedback: f64,
    pub mix: f64,
}

/// How a Mono sounds.
#[derive(Clone, Debug)]
pub struct MonoSound {
    pub wave: Wave,
    /// How much higher the second oscillator plays.
    pub detune_cents: f64,
    pub filter: Filter,
    /// Where the filter rests between notes, in hertz.
    pub cutoff: f64,
    /// Where each note opens it, and accented ones.
    pub peak: f64,
    pub accent_peak: f64,
    pub resonance: f64,
    pub filter_decay_ms: f64,
    pub drive: f64,
    pub fold: f64,
    pub decay_ms: f64,
    pub sustain: f64,
    pub release_ms: f64,
    pub echo: Option<Echo>,
    /// Bit depth and downsampling of a bitcrush after the distortion, if any.
    pub crush: Option<(f64, f64)>,
}

impl Default for MonoSound {
    fn default() -> MonoSound {
        MonoSound {
            wave: Wave::Saw,
            detune_cents: 10.0,
            filter: Filter::Lowpass,
            cutoff: 200.0,
            peak: 1300.0,
            accent_peak: 2400.0,
            resonance: 0.55,
            filter_decay_ms: 140.0,
            drive: 2.5,
            fold: 0.12,
            decay_ms: 70.0,
            sustain: 0.6,
            release_ms: 22.0,
            echo: None,
            crush: None,
        }
    }
}

/// Monophonic voice, for basses and leads: two oscillators through the
/// filter and the distortion into the VCA, with an optional echo after it.
/// Every note retunes them and opens the filter and the VCA. The sequencer,
/// its eight steps level, holds the filter away from there for sweeps.
pub struct Mono {
    stem: Stem,
    sound: MonoSound,
    ratio: f64,
}

impl Mono {
    pub fn new(track: &Track, name: &str) -> Mono {
        Mono::with(track, name, MonoSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: MonoSound) -> Mono {
        let ratio = 2f64.powf(sound.detune_cents / 1200.0);
        let (wave, filter) = (sound.wave.port(), sound.filter.port());
        let mut patch = Patcher::new(&format!(
            "Monophonic voice: two {wave}s {} cents apart through the {filter} filter and the \
             distortion. The score retunes them and opens the filter and the VCA on every note; \
             the sequencer, all its steps level, sweeps the filter on top of that.",
            sound.detune_cents
        ))
        .knob("osc 1", "freq", 55.0)
        .knob("osc 2", "freq", (55.0 * ratio * 1000.0).round() / 1000.0)
        .knob("filter", "cutoff", sound.cutoff)
        .knob("filter", "resonance", sound.resonance)
        .knob("dist", "drive", sound.drive)
        .knob("dist", "fold", sound.fold)
        .knob("vca", "gain", 0.0)
        .knob("master", "volume", 1.0)
        .cable(&format!("osc 1/{wave} -> filter/in"))
        .cable(&format!("osc 2/{wave} -> filter/in"))
        .cable(&format!("filter/{filter} -> dist/in"))
        .cable("seq/cv -> filter/cutoff cv");
        patch = match sound.crush {
            Some((bits, downsample)) => patch
                .knob("crush", "bits", bits)
                .knob("crush", "downsample", downsample)
                .cable("dist/out -> crush/in")
                .cable("crush/out -> vca/in"),
            None => patch.cable("dist/out -> vca/in"),
        };
        patch = match sound.echo {
            Some(echo) => patch
                .knob("delay", "time", echo.time)
                .knob("delay", "feedback", echo.feedback)
                .knob("delay", "mix", echo.mix)
                .cable("vca/out -> delay/in")
                .cable("delay/out -> master/in"),
            None => patch.cable("vca/out -> master/in"),
        };
        Mono {
            stem: Stem::new(track, name, patch.build()),
            sound,
            ratio,
        }
    }

    /// Plays `pitch` for `length` beats, then lets it go; an accent opens
    /// the filter wider.
    pub fn note(&mut self, at: f64, pitch: impl Pitch, length: f64, accent: bool) {
        self.note_at(at, pitch, length, accent, 1.0);
    }

    /// `note` at a level other than full.
    pub fn note_at(&mut self, at: f64, pitch: impl Pitch, length: f64, accent: bool, level: f64) {
        let clock = self.stem.clock();
        let sound = &self.sound;
        let f = pitch.hz();
        let retune = (at - clock.ms(6.0)).max(0.0);
        self.stem.set(retune, "osc 1/freq", f);
        self.stem.set(retune, "osc 2/freq", f * self.ratio);
        self.stem.set(at, "vca/gain", level);
        self.stem.glide(
            at + clock.ms(8.0),
            "vca/gain",
            sound.sustain * level,
            clock.ms(sound.decay_ms),
            3.0,
        );
        self.stem.glide(
            at + length,
            "vca/gain",
            0.0,
            clock.ms(sound.release_ms),
            2.0,
        );
        self.stem.set(
            at,
            "filter/cutoff",
            if accent {
                sound.accent_peak
            } else {
                sound.peak
            },
        );
        let settle = clock.ms(sound.filter_decay_ms);
        self.stem.glide(
            at + clock.ms(4.0),
            "filter/cutoff",
            sound.cutoff,
            settle,
            3.0,
        );
    }

    /// Plays the notes of a melody, as `Track::melody` makes them.
    pub fn notes(&mut self, notes: &[Note]) {
        for note in notes {
            self.note(note.at, note.pitch.as_str(), note.length, note.accent);
        }
    }

    /// Moves the whole filter, where notes open it and where it rests,
    /// `octaves` away from the sound's own (0) over `over` beats: a sweep.
    pub fn sweep(&mut self, at: f64, octaves: f64, over: f64, curve: f64) {
        // Every step holds the same voltage, which the cutoff adds to its
        // knob in normalized travel.
        let range = &filter::INFO.params[filter::CUTOFF];
        let offset = octaves / f64::from(range.max / range.min).log2();
        for step in 1..=sequencer::STEPS {
            self.stem
                .glide(at, &format!("seq/step {step}"), offset, over, curve);
        }
    }
}

impl Part for Mono {
    fn into_stems(self) -> Vec<Stem> {
        vec![self.stem]
    }
}

/// How a Chaos sounds; two of them a few hertz and milliseconds apart drift
/// away from each other.
#[derive(Clone, Debug)]
pub struct ChaosSound {
    pub delay_s: f64,
    pub lfo_rate: f64,
    pub osc2: f64,
    pub cutoff: f64,
    pub volume: f64,
}

impl Default for ChaosSound {
    fn default() -> ChaosSound {
        ChaosSound {
            delay_s: 0.09,
            lfo_rate: 5.0,
            osc2: 82.0,
            cutoff: 250.0,
            volume: 0.8,
        }
    }
}

/// The built-in chaos preset without its sequencer: two oscillators
/// frequency-modulating each other, folded, crushed and fed back through the
/// delay, retuned by random voltages. Silent until the score opens its VCA;
/// sweep it with knob moves ("filter/cutoff", "osc 1/fm depth",
/// "crush/bits", "delay/feedback"...).
pub struct Chaos {
    stem: Stem,
}

impl Chaos {
    pub fn new(track: &Track, name: &str) -> Chaos {
        Chaos::with(track, name, ChaosSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: ChaosSound) -> Chaos {
        let patch = Patcher::new(
            "The built-in chaos preset without its sequencer, which the score replaces: it \
             gates and sweeps the patch.",
        )
        .knob("atten", "gain a", 0.15)
        .knob("atten", "gain b", 0.05)
        .knob("atten", "gain c", 1.0)
        .knob("crush", "bits", 7.0)
        .knob("crush", "downsample", 2.0)
        .knob("delay", "feedback", 0.9)
        .knob("delay", "mix", 0.35)
        .knob("delay", "time", sound.delay_s)
        .knob("dist", "drive", 10.0)
        .knob("dist", "fold", 0.6)
        .knob("filter", "cutoff", sound.cutoff)
        .knob("filter", "resonance", 0.8)
        .knob("lfo", "rate", sound.lfo_rate)
        .knob("master", "volume", sound.volume)
        .knob("osc 1", "fm depth", 250.0)
        .knob("osc 1", "freq", 55.0)
        .knob("osc 2", "fm depth", 120.0)
        .knob("osc 2", "freq", sound.osc2)
        .knob("vca", "gain", 0.0)
        .cable("noise/white -> s&h/in")
        .cable("lfo/square -> s&h/trigger")
        .cable("s&h/out -> atten/a")
        .cable("atten/a -> osc 2/freq cv")
        .cable("lfo/sine -> atten/b")
        .cable("atten/b -> delay/time cv")
        .cable("osc 2/sine -> osc 1/fm")
        .cable("osc 1/square -> osc 2/fm")
        .cable("delay/out -> osc 1/fm")
        .cable("osc 1/saw -> dist/in")
        .cable("dist/out -> filter/in")
        .cable("filter/lowpass -> crush/in")
        .cable("crush/out -> delay/in")
        .cable("delay/out -> vca/in")
        .cable("vca/out -> master/in")
        .build();
        Chaos {
            stem: Stem::new(track, name, patch),
        }
    }

    /// Opens the VCA to `level` for `length` beats, then back to `bed`.
    pub fn stab(&mut self, at: f64, length: f64, level: f64, bed: f64) {
        let clock = self.stem.clock();
        self.stem.set(at, "vca/gain", level);
        self.stem.glide(
            at + length - clock.ms(30.0),
            "vca/gain",
            bed,
            clock.ms(45.0),
            2.0,
        );
    }

    /// Stabs on (time, length) notes, as `Track::gates` makes them.
    pub fn stabs(&mut self, notes: &[(f64, f64)], level: f64, bed: f64) {
        for &(at, length) in notes {
            self.stab(at, length, level, bed);
        }
    }
}

impl Part for Chaos {
    fn into_stems(self) -> Vec<Stem> {
        vec![self.stem]
    }
}

/// How a Drone sounds.
#[derive(Clone, Debug)]
pub struct DroneSound {
    /// The two saws, in hertz: by default a fifth on A.
    pub notes: (f64, f64),
    pub cutoff: f64,
    pub resonance: f64,
    pub drive: f64,
    pub fold: f64,
    pub delay_s: f64,
    pub feedback: f64,
    pub mix: f64,
    /// How fast and how far the low-pass sways.
    pub sway_hz: f64,
    pub sway: f64,
}

impl Default for DroneSound {
    fn default() -> DroneSound {
        DroneSound {
            notes: (55.0, 82.41),
            cutoff: 250.0,
            resonance: 0.45,
            drive: 1.6,
            fold: 0.35,
            delay_s: 0.375,
            feedback: 0.55,
            mix: 0.35,
            sway_hz: 0.07,
            sway: 0.08,
        }
    }
}

/// Two saws through a slowly swaying low-pass, a soft wavefolder and a long
/// delay, into the VCA. Silent until the score opens its VCA.
pub struct Drone {
    stem: Stem,
    notes: (f64, f64),
}

impl Drone {
    pub fn new(track: &Track, name: &str) -> Drone {
        Drone::with(track, name, DroneSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: DroneSound) -> Drone {
        let patch = Patcher::new(
            "Drone: two saws through a slowly swaying low-pass, a soft wavefolder and a long \
             delay. The score fades it, ducks it and retunes it.",
        )
        .knob("osc 1", "freq", sound.notes.0)
        .knob("osc 2", "freq", sound.notes.1)
        .knob("filter", "cutoff", sound.cutoff)
        .knob("filter", "resonance", sound.resonance)
        .knob("dist", "drive", sound.drive)
        .knob("dist", "fold", sound.fold)
        .knob("delay", "time", sound.delay_s)
        .knob("delay", "feedback", sound.feedback)
        .knob("delay", "mix", sound.mix)
        .knob("lfo", "rate", sound.sway_hz)
        .knob("atten", "gain b", sound.sway)
        .knob("vca", "gain", 0.0)
        .knob("master", "volume", 1.0)
        .cable("osc 1/saw -> filter/in")
        .cable("osc 2/saw -> filter/in")
        .cable("filter/lowpass -> dist/in")
        .cable("dist/out -> delay/in")
        .cable("delay/out -> vca/in")
        .cable("vca/out -> master/in")
        .cable("lfo/sine -> atten/b")
        .cable("atten/b -> filter/cutoff cv")
        .build();
        Drone {
            stem: Stem::new(track, name, patch),
            notes: sound.notes,
        }
    }

    /// The pitches it was built with, in hertz.
    pub fn notes(&self) -> (f64, f64) {
        self.notes
    }

    /// Moves the two saws to new pitches.
    pub fn tune(&mut self, at: f64, first: impl Pitch, second: impl Pitch) {
        self.stem.set(at, "osc 1/freq", first.hz());
        self.stem.set(at, "osc 2/freq", second.hz());
    }

    /// Slides the two saws to new pitches over `over` beats, at an even
    /// pace in semitones.
    pub fn slide(&mut self, at: f64, first: impl Pitch, second: impl Pitch, over: f64) {
        self.stem.glide(at, "osc 1/freq", first.hz(), over, 0.0);
        self.stem.glide(at, "osc 2/freq", second.hz(), over, 0.0);
    }

    /// Dips under each time (the kicks, say) and swells back to `level(time)`
    /// over 0.6 beats, like a sidechain.
    pub fn duck(&mut self, times: &[f64], level: impl Fn(f64) -> f64) {
        self.duck_with(times, level, 0.3, 0.6);
    }

    /// `duck`, dipping to `depth` times the level and swelling back over
    /// `release` beats.
    pub fn duck_with(
        &mut self,
        times: &[f64],
        level: impl Fn(f64) -> f64,
        depth: f64,
        release: f64,
    ) {
        let clock = self.stem.clock();
        for &t in times {
            let target = level(t);
            self.stem.set(t, "vca/gain", target * depth);
            self.stem
                .glide(t + clock.ms(15.0), "vca/gain", target, release, -1.0);
        }
    }
}

impl Part for Drone {
    fn into_stems(self) -> Vec<Stem> {
        vec![self.stem]
    }
}

/// Transitions: band-passed noise through the delay for risers and crashes,
/// and osc 1 through the attenuverter for sub drops.
pub struct Fx {
    stem: Stem,
    /// (time, level, length, top, bottom) of each sub drop.
    booms: Vec<(f64, f64, f64, f64, f64)>,
}

impl Fx {
    pub fn new(track: &Track, name: &str) -> Fx {
        let patch = Patcher::new(
            "Transitions: band-passed noise through the delay for the risers and crashes, and \
             osc 1 through the attenuverter for the sub drops on the downbeats.",
        )
        .knob("filter", "cutoff", 300.0)
        .knob("filter", "resonance", 0.7)
        .knob("delay", "time", 0.17)
        .knob("delay", "feedback", 0.5)
        .knob("delay", "mix", 0.4)
        .knob("osc 1", "freq", 40.0)
        .knob("atten", "gain a", 0.0)
        .knob("vca", "gain", 0.0)
        .knob("master", "volume", 1.0)
        .cable("noise/white -> filter/in")
        .cable("filter/bandpass -> delay/in")
        .cable("delay/out -> vca/in")
        .cable("vca/out -> master/in")
        .cable("atten/a -> master/in")
        .build();
        Fx {
            stem: Stem::new(track, name, patch),
            booms: Vec::new(),
        }
    }

    /// Noise swelling to `level` from `start` to `end` while its band climbs
    /// from 250 Hz to `top`.
    pub fn riser(&mut self, start: f64, end: f64, level: f64, top: f64) {
        self.stem.set(start, "filter/cutoff", 250.0);
        self.stem.set(start, "filter/resonance", 0.7);
        self.stem.glide(start, "vca/gain", level, end - start, -2.5);
        self.stem
            .glide(start, "filter/cutoff", top, end - start, -1.2);
        self.stem
            .glide(start, "filter/resonance", 0.85, end - start, 0.0);
    }

    /// A burst of bright noise dying out over `length` beats.
    pub fn crash(&mut self, at: f64, length: f64) {
        let clock = self.stem.clock();
        self.stem.set(at, "filter/cutoff", 7000.0);
        self.stem.set(at, "filter/resonance", 0.2);
        self.stem.set(at, "vca/gain", 0.5);
        self.stem
            .glide(at + clock.ms(10.0), "vca/gain", 0.0, length, 2.5);
    }

    /// Silences the noise at once.
    pub fn cut(&mut self, at: f64) {
        self.stem.set(at, "vca/gain", 0.0);
    }

    /// A sub drop: a sine falling from 160 Hz to 30 Hz over `length` beats.
    pub fn boom(&mut self, at: f64, level: f64, length: f64) {
        self.booms.push((at, level, length, 160.0, 30.0));
    }
}

impl Part for Fx {
    fn into_stems(mut self) -> Vec<Stem> {
        for &(at, _, length, top, bottom) in &self.booms {
            let ahead = self.stem.ahead(at);
            self.stem.set(ahead, "osc 1/freq", top);
            self.stem.glide(at, "osc 1/freq", bottom, length * 0.8, 2.0);
        }
        let strikes: Vec<_> = self
            .booms
            .iter()
            .map(|&(at, level, length, ..)| (at, level, length))
            .collect();
        self.stem
            .strike(&strikes, "osc 1/sine -> atten/a", "atten/gain a", 1.5);
        vec![self.stem]
    }
}

/// How a Metal sounds.
#[derive(Clone, Debug)]
pub struct MetalSound {
    /// The ring's pitch, in hertz.
    pub pitch: f64,
    /// The modulator's pitch as a multiple of `pitch`: away from whole
    /// numbers, the ring turns to metal.
    pub ratio: f64,
    /// How far the modulation swings the pitch, as a multiple of it.
    pub index: f64,
    /// How much band-passed noise strikes it, from 0 to 1.
    pub noise: f64,
    pub drive: f64,
    pub decay_ms: f64,
    pub curve: f64,
}

impl Default for MetalSound {
    fn default() -> MetalSound {
        MetalSound {
            pitch: 1400.0,
            ratio: 1.414,
            index: 1.2,
            noise: 0.35,
            drive: 1.8,
            decay_ms: 90.0,
            curve: 3.0,
        }
    }
}

/// Struck metal: osc 1 frequency-modulated by osc 2 at an inharmonic ratio,
/// with a burst of band-passed noise, through the distortion. Short decays
/// give picks on rock and rails, long ones bells. A hit's `tune` changes
/// its pitch.
pub struct Metal {
    stem: Stem,
    sound: MetalSound,
    hits: Vec<Hit>,
}

impl Metal {
    pub fn new(track: &Track, name: &str) -> Metal {
        Metal::with(track, name, MetalSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: MetalSound) -> Metal {
        let patch = Patcher::new(
            "Struck metal: osc 1 frequency-modulated by osc 2 at an inharmonic ratio, with a \
             burst of band-passed noise, through the distortion. Each hit plugs it into the VCA.",
        )
        .knob("osc 1", "freq", sound.pitch)
        .knob("osc 1", "fm depth", sound.pitch * sound.index)
        .knob("osc 2", "freq", sound.pitch * sound.ratio)
        .knob("atten", "gain a", sound.noise)
        .knob("filter", "cutoff", (sound.pitch * 2.0).min(18_000.0))
        .knob("filter", "resonance", 0.6)
        .knob("dist", "drive", sound.drive)
        .knob("dist", "fold", 0.0)
        .knob("vca", "gain", 0.0)
        .knob("master", "volume", 1.0)
        .cable("osc 2/sine -> osc 1/fm")
        .cable("osc 1/sine -> dist/in")
        .cable("noise/white -> atten/a")
        .cable("atten/a -> filter/in")
        .cable("filter/bandpass -> dist/in")
        .cable("vca/out -> master/in")
        .build();
        Metal {
            stem: Stem::new(track, name, patch),
            sound,
            hits: Vec::new(),
        }
    }
}

impl Part for Metal {
    fn into_stems(mut self) -> Vec<Stem> {
        let clock = self.stem.clock();
        let sound = &self.sound;
        // Retuned hits move the whole ring, in silence, before they strike.
        if self.hits.iter().any(|hit| hit.tune.is_some()) {
            for hit in &self.hits {
                let pitch = hit.tune.unwrap_or(sound.pitch);
                let ahead = self.stem.ahead(hit.at);
                self.stem.set(ahead, "osc 1/freq", pitch);
                self.stem.set(ahead, "osc 2/freq", pitch * sound.ratio);
                self.stem.set(ahead, "osc 1/fm depth", pitch * sound.index);
                self.stem
                    .set(ahead, "filter/cutoff", (pitch * 2.0).min(18_000.0));
            }
        }
        let strikes: Vec<_> = self
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.at,
                    hit.level,
                    clock.ms(hit.decay_ms.unwrap_or(sound.decay_ms)),
                )
            })
            .collect();
        self.stem
            .strike(&strikes, "dist/out -> vca/in", "vca/gain", sound.curve);
        vec![self.stem]
    }
}

/// How a Hum sounds.
#[derive(Clone, Debug)]
pub struct HumSound {
    /// The mains frequency, in hertz.
    pub mains: f64,
    /// Centre of the band it is heard through, in hertz.
    pub cutoff: f64,
    pub resonance: f64,
    pub drive: f64,
    pub bits: f64,
    /// How far the band sways, slowly.
    pub sway: f64,
}

impl Default for HumSound {
    fn default() -> HumSound {
        HumSound {
            mains: 50.0,
            cutoff: 420.0,
            resonance: 0.55,
            drive: 2.5,
            bits: 9.0,
            sway: 0.05,
        }
    }
}

/// Electrical hum: a square at the mains frequency and a saw on its third
/// harmonic, slightly sharp so that they beat, heard through a swaying band,
/// driven and crushed. Silent until the score opens its VCA.
pub struct Hum {
    stem: Stem,
}

/// The second oscillator plays the third harmonic, a little sharp.
const HUM_HARMONIC: f64 = 3.012;

impl Hum {
    pub fn new(track: &Track, name: &str) -> Hum {
        Hum::with(track, name, HumSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: HumSound) -> Hum {
        let patch = Patcher::new(
            "Electrical hum: a square at the mains frequency and a saw on its third harmonic, \
             slightly sharp so that they beat, through a swaying band-pass, driven and crushed.",
        )
        .knob("osc 1", "freq", sound.mains)
        .knob("osc 2", "freq", sound.mains * HUM_HARMONIC)
        .knob("filter", "cutoff", sound.cutoff)
        .knob("filter", "resonance", sound.resonance)
        .knob("dist", "drive", sound.drive)
        .knob("dist", "fold", 0.15)
        .knob("crush", "bits", sound.bits)
        .knob("crush", "downsample", 1.0)
        .knob("lfo", "rate", 0.23)
        .knob("atten", "gain b", sound.sway)
        .knob("vca", "gain", 0.0)
        .knob("master", "volume", 1.0)
        .cable("osc 1/square -> filter/in")
        .cable("osc 2/saw -> filter/in")
        .cable("filter/bandpass -> dist/in")
        .cable("dist/out -> crush/in")
        .cable("crush/out -> vca/in")
        .cable("vca/out -> master/in")
        .cable("lfo/sine -> atten/b")
        .cable("atten/b -> filter/cutoff cv")
        .build();
        Hum {
            stem: Stem::new(track, name, patch),
        }
    }

    /// Moves the hum's fundamental to `hz` over `over` beats, its harmonic
    /// with it.
    pub fn pitch(&mut self, at: f64, hz: f64, over: f64, curve: f64) {
        self.stem.glide(at, "osc 1/freq", hz, over, curve);
        self.stem
            .glide(at, "osc 2/freq", hz * HUM_HARMONIC, over, curve);
    }
}

impl Part for Hum {
    fn into_stems(self) -> Vec<Stem> {
        vec![self.stem]
    }
}

/// How a Pump sounds.
#[derive(Clone, Debug)]
pub struct PumpSound {
    /// The pitch of the thump, in hertz.
    pub thump: f64,
    /// Centre of the band of the breath, in hertz.
    pub hiss: f64,
    /// Feedback of the room the breath rings in.
    pub room: f64,
}

impl Default for PumpSound {
    fn default() -> PumpSound {
        PumpSound {
            thump: 62.0,
            hiss: 900.0,
            room: 0.45,
        }
    }
}

/// A pump at work: each stroke is a low thump and a breath of band-passed
/// noise ringing in a room.
pub struct Pump {
    stem: Stem,
    sound: PumpSound,
    /// (time, level) of each stroke's thump.
    thumps: Vec<(f64, f64)>,
}

impl Pump {
    pub fn new(track: &Track, name: &str) -> Pump {
        Pump::with(track, name, PumpSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: PumpSound) -> Pump {
        let patch = Patcher::new(
            "A pump at work: a sine thump through the attenuverter, and a breath of band-passed \
             noise through the delay into the VCA.",
        )
        .knob("osc 1", "freq", sound.thump)
        .knob("atten", "gain a", 0.0)
        .knob("filter", "cutoff", sound.hiss)
        .knob("filter", "resonance", 0.35)
        .knob("delay", "time", 0.23)
        .knob("delay", "feedback", sound.room)
        .knob("delay", "mix", 0.4)
        .knob("vca", "gain", 0.0)
        .knob("master", "volume", 1.0)
        .cable("atten/a -> master/in")
        .cable("noise/white -> filter/in")
        .cable("filter/bandpass -> delay/in")
        .cable("delay/out -> vca/in")
        .cable("vca/out -> master/in")
        .build();
        Pump {
            stem: Stem::new(track, name, patch),
            sound,
            thumps: Vec::new(),
        }
    }

    /// One stroke of `length` beats: the thump, then the breath swelling and
    /// dying away.
    pub fn stroke(&mut self, at: f64, level: f64, length: f64) {
        self.thumps.push((at, level));
        self.breath(at, level * 0.5, length);
    }

    /// The breath alone, without the thump: it swells to `level` over 30 %
    /// of `length` beats, then dies away over the next 60 %.
    pub fn breath(&mut self, at: f64, level: f64, length: f64) {
        self.stem.glide(at, "vca/gain", level, length * 0.3, -1.0);
        self.stem
            .glide(at + length * 0.3, "vca/gain", 0.0, length * 0.6, 2.0);
    }
}

impl Part for Pump {
    fn into_stems(mut self) -> Vec<Stem> {
        let clock = self.stem.clock();
        for &(at, _) in &self.thumps {
            let ahead = self.stem.ahead(at);
            self.stem.set(ahead, "osc 1/freq", self.sound.thump * 1.8);
            self.stem
                .glide(at, "osc 1/freq", self.sound.thump, clock.ms(120.0), 4.0);
        }
        let strikes: Vec<_> = self
            .thumps
            .iter()
            .map(|&(at, level)| (at, level, clock.ms(260.0)))
            .collect();
        self.stem
            .strike(&strikes, "osc 1/sine -> atten/a", "atten/gain a", 2.5);
        vec![self.stem]
    }
}

/// How a Space sounds.
#[derive(Clone, Debug)]
pub struct SpaceSound {
    /// Centre of the band the tail is heard in, in hertz.
    pub cutoff: f64,
    pub resonance: f64,
    /// How many times a second the noise is drawn anew, in hertz: two
    /// spaces with different grains are independent, for the two sides of
    /// a room.
    pub grain: f64,
    /// How long the tail takes to swell after a hit.
    pub bloom_ms: f64,
    /// How long it takes to die away, about 60 dB.
    pub decay_ms: f64,
}

impl Default for SpaceSound {
    fn default() -> SpaceSound {
        SpaceSound {
            cutoff: 400.0,
            resonance: 0.15,
            grain: 3000.0,
            bloom_ms: 80.0,
            decay_ms: 4000.0,
        }
    }
}

/// The tail of a large room, without a single echo: white noise drawn anew
/// `grain` times a second, band-passed, swelling after each hit and dying
/// away while it darkens an octave, as the highs of a real room die first.
/// Hit it with the drums it carries; a hit's `tune` moves its band.
pub struct Space {
    stem: Stem,
    sound: SpaceSound,
    hits: Vec<Hit>,
}

impl Space {
    pub fn new(track: &Track, name: &str) -> Space {
        Space::with(track, name, SpaceSound::default())
    }

    pub fn with(track: &Track, name: &str, sound: SpaceSound) -> Space {
        let patch = Patcher::new(
            "A room's tail: white noise sampled and held at the pitch of osc 1, band-passed, into \
             the VCA. The score swells it after each hit, then lets it die away as it darkens.",
        )
        .knob("osc 1", "freq", sound.grain)
        .knob("filter", "cutoff", sound.cutoff)
        .knob("filter", "resonance", sound.resonance)
        .knob("vca", "gain", 0.0)
        .knob("master", "volume", 1.0)
        .cable("noise/white -> s&h/in")
        .cable("osc 1/square -> s&h/trigger")
        .cable("s&h/out -> filter/in")
        .cable("filter/bandpass -> vca/in")
        .cable("vca/out -> master/in")
        .build();
        Space {
            stem: Stem::new(track, name, patch),
            sound,
            hits: Vec::new(),
        }
    }
}

impl Part for Space {
    fn into_stems(mut self) -> Vec<Stem> {
        let clock = self.stem.clock();
        let bloom = clock.ms(self.sound.bloom_ms);
        let mut hits = self.hits.clone();
        hits.sort_by(|a, b| a.at.total_cmp(&b.at));
        for (i, hit) in hits.iter().enumerate() {
            let band = hit.tune.unwrap_or(self.sound.cutoff);
            let decay = clock.ms(hit.decay_ms.unwrap_or(self.sound.decay_ms));
            self.stem.set(hit.at, "filter/cutoff", band);
            self.stem
                .glide(hit.at, "filter/cutoff", band / 2.0, bloom + decay, 0.0);
            self.stem.glide(hit.at, "vca/gain", hit.level, bloom, 2.0);
            // A hit during the swell takes it over.
            if hits.get(i + 1).is_none_or(|next| next.at > hit.at + bloom) {
                self.stem.glide(hit.at + bloom, "vca/gain", 0.0, decay, 6.9);
            }
        }
        vec![self.stem]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_open_then_release() {
        let t = Track::new(60, 1);
        let mut bass = Mono::new(&t, "bass");
        bass.note(1.0, "A1", 0.5, false);
        let stem = &bass.into_stems()[0];
        let moves = |knob: &str| -> Vec<(f64, f64)> {
            stem.events()
                .iter()
                .filter(|e| e.knob.as_deref() == Some(knob))
                .map(|e| (e.at, e.value.unwrap()))
                .collect()
        };
        assert_eq!(moves("vca/gain")[0], (1.0, 1.0));
        assert_eq!(moves("vca/gain").last(), Some(&(1.5, 0.0)));
        assert_eq!(moves("osc 1/freq"), [(0.994, 55.0)]);
    }

    #[test]
    fn drums_hold_their_hits_until_added() {
        let t = Track::new(120, 2);
        let mut kick = Kick::new(&t, "kick");
        kick.hits(t.steps("x...", 0, 1));
        kick.hit(t.bar(1), 1.0).decay_ms(900.0).tune(45.0);
        assert_eq!(kick.times(), [0.0, 1.0, 2.0, 3.0, 4.0]);
        let stems = kick.into_stems();
        // Per hit: the pitch up and down, then the level, plug, fall and pull.
        assert_eq!(stems[0].events().len(), 5 * 6);
        let last_pitch = stems[0]
            .events()
            .iter()
            .rfind(|e| e.ramp > 0.0 && e.knob.as_deref() == Some("osc 1/freq"));
        assert_eq!(last_pitch.unwrap().value, Some(45.0));
    }

    #[test]
    fn hats_go_through_the_crusher_when_asked() {
        let t = Track::new(120, 1);
        let sound = HatSound {
            crush: Some((9.0, 1.4)),
            ..HatSound::default()
        };
        let mut hat = Hat::with(&t, "ohat", sound);
        hat.hit(0.5, 1.0);
        let stem = &hat.into_stems()[0];
        assert!(
            stem.events()
                .iter()
                .any(|e| e.connect.as_deref() == Some("crush/out -> vca/in"))
        );
    }

    fn values(stem: &Stem, knob: &str) -> Vec<(f64, f64)> {
        stem.events()
            .iter()
            .filter(|e| e.knob.as_deref() == Some(knob))
            .map(|e| (e.at, e.value.unwrap()))
            .collect()
    }

    #[test]
    fn retuned_metal_moves_its_whole_ring() {
        let t = Track::new(60, 1);
        let mut metal = Metal::new(&t, "picks");
        metal.hit(0.5, 1.0).tune(1000.0);
        metal.hit(1.0, 1.0);
        let stem = &metal.into_stems()[0];
        let pitches: Vec<f64> = values(stem, "osc 2/freq").iter().map(|v| v.1).collect();
        assert_eq!(pitches, [1414.0, 1979.6]);
        assert_eq!(
            stem.events().iter().filter(|e| e.connect.is_some()).count(),
            2
        );
    }

    #[test]
    fn pumps_thump_and_breathe() {
        let t = Track::new(60, 2);
        let mut pump = Pump::new(&t, "pump");
        pump.stroke(1.0, 0.8, 2.0);
        let stem = &pump.into_stems()[0];
        assert_eq!(values(stem, "vca/gain"), [(1.0, 0.4), (1.6, 0.0)]);
        let thump = Some("osc 1/sine -> atten/a");
        assert!(stem.events().iter().any(|e| e.connect.as_deref() == thump));
    }

    #[test]
    fn a_breath_has_no_thump() {
        let t = Track::new(60, 2);
        let mut pump = Pump::new(&t, "pump");
        pump.breath(1.0, 0.4, 2.0);
        let stem = &pump.into_stems()[0];
        assert_eq!(values(stem, "vca/gain"), [(1.0, 0.4), (1.6, 0.0)]);
        assert!(stem.events().iter().all(|e| e.connect.is_none()));
    }

    #[test]
    fn a_space_swells_then_dies_away() {
        // A beat lasts a second: the swell is 0.08 beats, the decay 4.
        let t = Track::new(60, 4);
        let mut room = Space::new(&t, "room");
        room.hit(1.0, 0.8);
        room.hit(1.05, 1.0).tune(800.0);
        room.hit(3.0, 0.5);
        let stem = &room.into_stems()[0];
        let gains: Vec<(f64, f64, f64)> = stem
            .events()
            .iter()
            .filter(|e| e.knob.as_deref() == Some("vca/gain"))
            .map(|e| (e.at, e.value.unwrap(), e.ramp))
            .collect();
        // The second hit comes during the first one's swell and takes over.
        assert_eq!(
            gains,
            [
                (1.0, 0.8, 0.08),
                (1.05, 1.0, 0.08),
                (1.13, 0.0, 4.0),
                (3.0, 0.5, 0.08),
                (3.08, 0.0, 4.0)
            ]
        );
        let bands = values(stem, "filter/cutoff");
        assert_eq!(bands[2..4], [(1.05, 800.0), (1.05, 400.0)]);
    }

    #[test]
    fn kicks_can_be_muffled() {
        let t = Track::new(60, 1);
        let sound = KickSound {
            lowpass: Some(300.0),
            ..KickSound::default()
        };
        let stem = &Kick::with(&t, "kick", sound).into_stems()[0];
        let crate::compose::Source::Patch(patch) = &stem.source else {
            panic!("an instrument builds its patch");
        };
        let cable =
            |from: &str, to: &str| patch.cables.iter().any(|c| c.from == from && c.to == to);
        assert!(cable("dist", "filter") && cable("filter", "master"));
        assert!(!cable("dist", "master"));
    }

    #[test]
    fn sweeps_move_the_filter_by_octaves() {
        let t = Track::new(60, 1);
        let mut lead = Mono::new(&t, "lead");
        lead.sweep(0.5, 1.0, 0.0, 0.0);
        let stem = &lead.into_stems()[0];
        let steps: Vec<f64> = stem
            .events()
            .iter()
            .filter(|e| e.knob.as_deref().is_some_and(|k| k.starts_with("seq/step")))
            .map(|e| e.value.unwrap())
            .collect();
        assert_eq!(steps.len(), 8);
        let cutoff = &filter::INFO.params[filter::CUTOFF];
        let swept = cutoff.from_normalized(cutoff.to_normalized(1000.0) + steps[0] as f32);
        assert!((swept / 2000.0 - 1.0).abs() < 1e-3, "{swept}");
    }

    #[test]
    fn an_unswept_voice_sounds_as_without_the_sequencer() {
        let t = Track::new(60, 1);
        let stem = &Mono::new(&t, "lead").into_stems()[0];
        let crate::compose::Source::Patch(patch) = &stem.source else {
            panic!("an instrument builds its patch");
        };
        let mut open = patch.clone();
        open.knobs
            .entry("vca".into())
            .or_default()
            .insert("gain".into(), 1.0);
        let mut plain = open.clone();
        plain.cables.retain(|c| c.from != "seq");
        let render = |patch: &audiochaos::patch::Patch| {
            let (mut graph, out) = audiochaos::rack::build(48_000.0, patch).unwrap();
            let mut audio = Vec::new();
            for _ in 0..200 {
                graph.process();
                audio.extend_from_slice(graph.output(out, audiochaos::modules::master::OUT));
            }
            audio
        };
        let swept = render(&open);
        assert!(swept.iter().any(|s| s.abs() > 0.1), "the voice plays");
        assert_eq!(swept, render(&plain));
    }

    #[test]
    fn the_hum_keeps_its_harmonic_when_it_moves() {
        let t = Track::new(60, 1);
        let mut hum = Hum::new(&t, "cable");
        hum.pitch(0.0, 100.0, 1.0, 0.0);
        let stem = &hum.into_stems()[0];
        assert_eq!(values(stem, "osc 2/freq"), [(0.0, 301.2)]);
    }

    #[test]
    fn monos_can_be_crushed() {
        let t = Track::new(60, 1);
        let sound = MonoSound {
            crush: Some((5.0, 3.0)),
            ..MonoSound::default()
        };
        let stem = &Mono::with(&t, "wired", sound).into_stems()[0];
        let crate::compose::Source::Patch(patch) = &stem.source else {
            panic!("an instrument builds its patch");
        };
        assert!(
            patch
                .cables
                .iter()
                .any(|c| c.from == "crush" && c.to == "vca")
        );
        assert!(
            !patch
                .cables
                .iter()
                .any(|c| c.from == "dist" && c.to == "vca")
        );
    }
}
