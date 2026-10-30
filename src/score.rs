//! Scores: what the renderer plays, as written in track.json. Every stem is
//! the stock audiochaos rack set up with a patch and driven by timed knob
//! moves and cable changes; the stems are then mixed to stereo.

use std::f32::consts::FRAC_PI_4;
use std::fs;
use std::path::Path;
use std::thread;

use audiochaos::engine::graph::{Cable, Graph, NodeId};
use audiochaos::engine::module::BLOCK;
use audiochaos::modules::master;
use audiochaos::patch::Patch;
use audiochaos::rack;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Score {
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    /// When set, the length and every event time and ramp are in beats at
    /// this tempo instead of seconds.
    #[serde(default)]
    pub bpm: Option<f64>,
    pub length: f64,
    /// Anything the tools around the renderer keep with the score, such as
    /// section markers. The render ignores it.
    #[serde(default)]
    pub meta: serde_json::Value,
    pub stems: Vec<ScoreStem>,
}

fn default_sample_rate() -> u32 {
    48_000
}

/// The rack's knobs glide from their defaults to the patch over a few
/// milliseconds. Running it this long before the render starts keeps that
/// glide out, so no stem begins with a blip.
const SETTLE_SECONDS: f64 = 0.1;

/// One rack, set up with a patch and played by its events.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreStem {
    pub name: String,
    /// A built-in preset name, or a patch file relative to the score file.
    pub patch: String,
    #[serde(default)]
    pub gain_db: f32,
    /// From -1 (left) to 1 (right).
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub events: Vec<Event>,
}

/// A change at a given time, exactly one of: a knob turned to `value`, at
/// once or gradually over `ramp`; a cable plugged; a cable pulled.
///
/// Changes land on the nearest block boundary (64 samples), and knobs still
/// glide through the rack's 5 ms smoothing, so a hard attack comes from
/// plugging a cable rather than from turning a knob.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub at: f64,
    /// `"module/knob"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knob: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ramp: f64,
    /// Shape of the ramp, in knob travel: 0 is a straight line, positive
    /// moves fast then settles (decays), negative starts slow (risers).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub curve: f64,
    /// `"module/output -> module/input"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disconnect: Option<String>,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

/// The file a stem is written to: its name, made safe.
pub fn file_name(stem: &str) -> String {
    let name: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    name + ".wav"
}

enum Op {
    Knob {
        node: NodeId,
        param: usize,
        value: f32,
        /// In blocks; 0 jumps.
        ramp: u64,
        curve: f32,
    },
    Connect(Cable),
    Disconnect(Cable),
}

struct Change {
    block: u64,
    op: Op,
    /// Index in the stem's events, for error messages.
    event: usize,
}

/// A knob travelling between two positions, in normalized knob space, moved
/// once per block.
struct Ramp {
    node: NodeId,
    param: usize,
    from: f32,
    to: f32,
    start: u64,
    end: u64,
    curve: f32,
}

impl Ramp {
    /// Moves the knob to where it should be at `block`; false once arrived.
    fn advance(&self, graph: &mut Graph, block: u64) -> bool {
        let x = ((block - self.start) as f32 / (self.end - self.start) as f32).min(1.0);
        let position = self.from + (self.to - self.from) * shape(x, self.curve);
        let value = graph.info(self.node).params[self.param].from_normalized(position);
        graph.set_param(self.node, self.param, value);
        x < 1.0
    }
}

/// Ramp progress 0..1 to knob travel 0..1.
fn shape(x: f32, curve: f32) -> f32 {
    if curve.abs() < 1e-3 {
        x
    } else {
        (1.0 - (-curve * x).exp()) / (1.0 - (-curve).exp())
    }
}

/// The rack's distortions clip lopsided waves and leave some DC, which would
/// thump wherever a stem stops short: like a mixing desk, every stem is
/// high-passed at 10 Hz.
fn block_dc(audio: &mut [f32], sample_rate: u32) {
    let pole = 1.0 - 2.0 * std::f64::consts::PI * 10.0 / f64::from(sample_rate);
    let (mut last_in, mut last_out) = (0.0f64, 0.0f64);
    for sample in audio {
        let x = f64::from(*sample);
        last_out = x - last_in + pole * last_out;
        last_in = x;
        *sample = last_out as f32;
    }
}

impl Score {
    pub fn from_json(json: &str) -> Result<Score, String> {
        let score: Score = serde_json::from_str(json).map_err(|e| format!("invalid score: {e}"))?;
        if !(score.length > 0.0 && score.length.is_finite()) {
            return Err("the score needs a positive length".into());
        }
        if score.bpm.is_some_and(|bpm| !(bpm > 0.0 && bpm.is_finite())) {
            return Err("the bpm must be positive".into());
        }
        if score.sample_rate == 0 {
            return Err("the sample rate must be positive".into());
        }
        Ok(score)
    }

    /// One patch (a file or a preset) left to play on its own, in seconds.
    pub fn single(patch: &str, seconds: f64) -> Score {
        Score {
            sample_rate: default_sample_rate(),
            bpm: None,
            length: seconds,
            meta: serde_json::Value::Null,
            stems: vec![ScoreStem {
                name: "sound".into(),
                patch: patch.into(),
                gain_db: 0.0,
                pan: 0.0,
                events: Vec::new(),
            }],
        }
    }

    fn seconds_of(&self, t: f64) -> f64 {
        self.bpm.map_or(t, |bpm| t * 60.0 / bpm)
    }

    pub fn seconds(&self) -> f64 {
        self.seconds_of(self.length)
    }

    pub fn samples(&self) -> usize {
        (self.seconds() * f64::from(self.sample_rate)).round() as usize
    }

    /// The block at whose start something happening at `t` takes effect.
    fn block_of(&self, t: f64) -> u64 {
        (self.seconds_of(t) * f64::from(self.sample_rate) / BLOCK as f64).round() as u64
    }

    /// Renders every stem, each on its own thread, in stem order.
    pub fn render(&self, dir: &Path) -> Result<Vec<Vec<f32>>, String> {
        thread::scope(|s| {
            let handles: Vec<_> = self
                .stems
                .iter()
                .map(|stem| {
                    s.spawn(move || {
                        self.render_stem(stem, dir)
                            .map_err(|e| format!("stem {:?}: {e}", stem.name))
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("a render thread panicked"))
                .collect()
        })
    }

    fn render_stem(&self, stem: &ScoreStem, dir: &Path) -> Result<Vec<f32>, String> {
        let patch = load_patch(&stem.patch, dir)?;
        let (mut graph, out) =
            rack::build(self.sample_rate as f32, &patch).map_err(|e| e.to_string())?;
        let changes = self.schedule(&graph, &stem.events)?;
        let settle = SETTLE_SECONDS * f64::from(self.sample_rate) / BLOCK as f64;
        for _ in 0..settle.ceil() as usize {
            graph.process();
        }
        let mut changes = changes.iter().peekable();
        let mut ramps = Vec::new();
        let samples = self.samples();
        let mut audio = Vec::with_capacity(samples + BLOCK);
        let mut block = 0;
        while audio.len() < samples {
            while let Some(change) = changes.next_if(|c| c.block <= block) {
                apply(&mut graph, &mut ramps, change).map_err(|e| {
                    let at = stem.events[change.event].at;
                    format!("event {} (at {at}): {e}", change.event)
                })?;
            }
            ramps.retain(|ramp| ramp.advance(&mut graph, block));
            graph.process();
            audio.extend_from_slice(graph.output(out, master::OUT));
            block += 1;
        }
        audio.truncate(samples);
        block_dc(&mut audio, self.sample_rate);
        Ok(audio)
    }

    /// Checks and resolves every event, sorted by block. The sort is stable,
    /// so simultaneous events apply in file order.
    fn schedule(&self, graph: &Graph, events: &[Event]) -> Result<Vec<Change>, String> {
        let mut changes = events
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let op = self
                    .op(graph, e)
                    .map_err(|err| format!("event {i} (at {}): {err}", e.at))?;
                Ok(Change {
                    block: self.block_of(e.at),
                    op,
                    event: i,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        changes.sort_by_key(|c| c.block);
        Ok(changes)
    }

    fn op(&self, graph: &Graph, e: &Event) -> Result<Op, String> {
        if !(e.at >= 0.0 && e.at.is_finite()) {
            return Err("the time must be zero or more".into());
        }
        match (&e.knob, &e.connect, &e.disconnect) {
            (Some(knob), None, None) => {
                let value = e.value.ok_or("a knob event needs a value")?;
                if !(e.ramp >= 0.0 && e.ramp.is_finite()) {
                    return Err("the ramp must be zero or more".into());
                }
                let (node, param) = knob_of(graph, knob)?;
                Ok(Op::Knob {
                    node,
                    param,
                    value: value as f32,
                    ramp: self.block_of(e.at + e.ramp) - self.block_of(e.at),
                    curve: e.curve as f32,
                })
            }
            (None, Some(cable), None) | (None, None, Some(cable)) => {
                if e.value.is_some() || e.ramp != 0.0 || e.curve != 0.0 {
                    return Err("value, ramp and curve only go with a knob".into());
                }
                let cable = cable_of(graph, cable)?;
                Ok(if e.connect.is_some() {
                    Op::Connect(cable)
                } else {
                    Op::Disconnect(cable)
                })
            }
            _ => Err("an event takes exactly one of knob, connect and disconnect".into()),
        }
    }

    /// Interleaved stereo: each stem at its gain, panned at constant power.
    pub fn mix(&self, stems: &[Vec<f32>]) -> Vec<f32> {
        let mut mix = vec![0.0; 2 * self.samples()];
        for (stem, audio) in self.stems.iter().zip(stems) {
            let gain = 10f32.powf(stem.gain_db / 20.0);
            let angle = (stem.pan.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
            let (left, right) = (gain * angle.cos(), gain * angle.sin());
            for (frame, s) in mix.as_chunks_mut::<2>().0.iter_mut().zip(audio) {
                frame[0] += s * left;
                frame[1] += s * right;
            }
        }
        mix
    }
}

fn apply(graph: &mut Graph, ramps: &mut Vec<Ramp>, change: &Change) -> Result<(), String> {
    match change.op {
        Op::Knob {
            node,
            param,
            value,
            ramp,
            curve,
        } => {
            // A new move on a knob replaces the one in progress.
            ramps.retain(|r| (r.node, r.param) != (node, param));
            if ramp == 0 {
                graph.set_param(node, param, value);
            } else {
                let info = &graph.info(node).params[param];
                ramps.push(Ramp {
                    node,
                    param,
                    from: info.to_normalized(graph.param(node, param)),
                    to: info.to_normalized(value),
                    start: change.block,
                    end: change.block + ramp,
                    curve,
                });
            }
        }
        Op::Connect(cable) => graph.connect(cable).map_err(|e| e.to_string())?,
        Op::Disconnect(cable) => {
            if !graph.disconnect(cable) {
                return Err("that cable is not plugged".into());
            }
        }
    }
    Ok(())
}

fn load_patch(name: &str, dir: &Path) -> Result<Patch, String> {
    if let Some(patch) = rack::preset(name) {
        return Ok(patch);
    }
    let path = dir.join(name);
    let json = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read patch {}: {e}", path.display()))?;
    Patch::from_json(&json).map_err(|e| format!("{}: {e}", path.display()))
}

fn node_of(graph: &Graph, name: &str) -> Result<NodeId, String> {
    graph
        .nodes()
        .find(|&n| graph.name(n) == name)
        .ok_or_else(|| format!("no module named {name:?}"))
}

/// Splits `"module/name"`.
fn split(path: &str) -> Result<(&str, &str), String> {
    path.split_once('/')
        .map(|(module, name)| (module.trim(), name.trim()))
        .ok_or_else(|| format!("{path:?} is not \"module/name\""))
}

fn knob_of(graph: &Graph, path: &str) -> Result<(NodeId, usize), String> {
    let (module, knob) = split(path)?;
    let node = node_of(graph, module)?;
    let param = graph
        .info(node)
        .params
        .iter()
        .position(|p| p.name == knob)
        .ok_or_else(|| format!("module {module:?} has no knob {knob:?}"))?;
    Ok((node, param))
}

fn cable_of(graph: &Graph, spec: &str) -> Result<Cable, String> {
    let (from, to) = spec
        .split_once("->")
        .ok_or_else(|| format!("{spec:?} is not \"module/output -> module/input\""))?;
    let ((from_module, output), (to_module, input)) = (split(from)?, split(to)?);
    let (from, to) = (node_of(graph, from_module)?, node_of(graph, to_module)?);
    let from_port = graph
        .info(from)
        .outputs
        .iter()
        .position(|o| *o == output)
        .ok_or_else(|| format!("module {from_module:?} has no output {output:?}"))?;
    let to_info = graph.info(to);
    let to_port = (0..to_info.num_input_ports())
        .find(|&port| to_info.input_port_name(port) == input)
        .ok_or_else(|| format!("module {to_module:?} has no input {input:?}"))?;
    Ok(Cable {
        from,
        from_port,
        to,
        to_port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: usize = 48_000;
    const ONE_SECOND: &str = r#""length": 1.0"#;

    /// The `bass` preset (a gated, sequenced saw) played with `events`.
    /// `head` holds the score's other fields.
    fn render(head: &str, events: &str) -> Result<Vec<f32>, String> {
        let json = format!(
            r#"{{ {head}, "stems": [{{ "name": "bass", "patch": "bass", "events": [{events}] }}] }}"#
        );
        let track = Score::from_json(&json)?;
        Ok(track.render(Path::new("."))?.remove(0))
    }

    fn loudest(audio: &[f32], from: f32, to: f32) -> f32 {
        let range = (from * SR as f32) as usize..(to * SR as f32) as usize;
        audio[range].iter().fold(0.0, |m, s| m.max(s.abs()))
    }

    #[test]
    fn the_desk_takes_the_dc_out() {
        let sine = |n: usize| (2.0 * std::f32::consts::PI * 100.0 * n as f32 / SR as f32).sin();
        let mut audio: Vec<f32> = (0..SR).map(|n| 0.2 + sine(n)).collect();
        block_dc(&mut audio, SR as u32);
        let tail = &audio[SR / 2..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 1e-3, "{mean}");
        assert!(
            (loudest(&audio, 0.5, 1.0) - 1.0).abs() < 0.01,
            "a 100 Hz tone goes through"
        );
    }

    #[test]
    fn curves_start_and_end_on_the_ramp_ends() {
        for curve in [-4.0, 0.0, 4.0] {
            assert!(shape(0.0, curve).abs() < 1e-6);
            assert!((shape(1.0, curve) - 1.0).abs() < 1e-6);
        }
        assert!(shape(0.5, 4.0) > 0.5, "positive curves move fast first");
        assert!(shape(0.5, -4.0) < 0.5, "negative curves start slow");
    }

    #[test]
    fn knob_events_land_at_their_time() {
        let audio = render(
            ONE_SECOND,
            r#"{ "at": 0, "knob": "master/volume", "value": 0 },
               { "at": 0.5, "knob": "master/volume", "value": 1 }"#,
        )
        .unwrap();
        assert!(loudest(&audio, 0.1, 0.49) < 1e-3);
        assert!(loudest(&audio, 0.6, 1.0) > 0.1);
    }

    #[test]
    fn times_can_be_in_beats() {
        // At 120 bpm, beat 1 is half a second in.
        let audio = render(
            r#""bpm": 120, "length": 2"#,
            r#"{ "at": 0, "knob": "master/volume", "value": 0 },
               { "at": 1, "knob": "master/volume", "value": 1 }"#,
        );
        let audio = audio.unwrap();
        assert!(loudest(&audio, 0.1, 0.49) < 1e-3);
        assert!(loudest(&audio, 0.6, 1.0) > 0.1);
    }

    #[test]
    fn ramps_reach_their_value_gradually() {
        let audio = render(
            ONE_SECOND,
            r#"{ "at": 0, "knob": "master/volume", "value": 0 },
               { "at": 0.1, "knob": "master/volume", "value": 1, "ramp": 0.8 }"#,
        )
        .unwrap();
        let (early, late) = (loudest(&audio, 0.1, 0.3), loudest(&audio, 0.8, 1.0));
        assert!(early > 0.0 && early < late / 2.0, "{early} then {late}");
    }

    #[test]
    fn cables_can_be_pulled_and_plugged() {
        let audio = render(
            ONE_SECOND,
            r#"{ "at": 0, "disconnect": "vca/out -> master/in" },
               { "at": 0.5, "connect": "vca/out -> master/in" }"#,
        )
        .unwrap();
        assert!(loudest(&audio, 0.0, 0.49) < 1e-3);
        assert!(loudest(&audio, 0.6, 1.0) > 0.1);
    }

    #[test]
    fn mistakes_are_reported_with_their_event() {
        let error = |events: &str| render(ONE_SECOND, events).unwrap_err();
        assert!(error(r#"{ "at": 0, "knob": "vca/nope", "value": 1 }"#).contains("no knob"));
        assert!(error(r#"{ "at": 0, "knob": "osc 9/freq", "value": 1 }"#).contains("no module"));
        assert!(error(r#"{ "at": 0, "connect": "seq/cv -> vca" }"#).contains("module/name"));
        assert!(error(r#"{ "at": 0, "knob": "vca/gain" }"#).contains("needs a value"));
        assert!(
            error(r#"{ "at": 0, "knob": "vca/gain", "value": 1, "connect": "a/b -> c/d" }"#)
                .contains("exactly one")
        );
        let unplugged = error(r#"{ "at": 0.5, "disconnect": "noise/white -> vca/in" }"#);
        assert!(unplugged.contains("event 0 (at 0.5)"), "{unplugged}");
        assert!(unplugged.contains("not plugged"), "{unplugged}");
    }

    #[test]
    fn starts_from_the_patch_not_from_the_defaults() {
        // The VCA's default gain is 1: without settling, the saw would leak
        // out while the gain glides down to the patch's 0.
        let patch = r#"{ "version": 1, "knobs": { "vca": { "gain": 0.0 } }, "cables": [
            { "from": "osc 1", "output": "saw", "to": "vca", "input": "in" },
            { "from": "vca", "output": "out", "to": "master", "input": "in" } ] }"#;
        let path = std::env::temp_dir().join("audiochaos-studio-settle-test.json");
        fs::write(&path, patch).unwrap();
        let json = format!(
            r#"{{ "length": 0.1, "stems": [{{ "name": "saw", "patch": {:?} }}] }}"#,
            path.to_str().unwrap()
        );
        let audio = Score::from_json(&json).unwrap().render(Path::new("."));
        fs::remove_file(&path).unwrap();
        assert!(loudest(&audio.unwrap()[0], 0.0, 0.1) < 1e-4);
    }

    #[test]
    fn meta_is_left_to_the_tools() {
        let head = r#""length": 1.0, "meta": { "markers": { "intro": 0 }, "anything": [1, "a"] }"#;
        assert_eq!(render(head, "").unwrap().len(), SR);
        assert!(render(r#""length": 1.0, "metta": {}"#, "").is_err());
    }

    #[test]
    fn stems_are_panned_at_constant_power() {
        let json = r#"{ "length": 0.1, "stems": [
            { "name": "left", "patch": "bass", "pan": -1 },
            { "name": "middle", "patch": "bass", "pan": 0, "gain_db": -6 } ] }"#;
        let track = Score::from_json(json).unwrap();
        let mix = track.mix(&[vec![1.0; 4800], vec![1.0; 4800]]);
        let half = 10f32.powf(-6.0 / 20.0) * FRAC_PI_4.cos();
        assert!((mix[0] - (1.0 + half)).abs() < 1e-6);
        assert!((mix[1] - half).abs() < 1e-6);
    }
}
