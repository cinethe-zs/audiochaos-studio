# audiochaos-studio

A studio for making music with [audiochaos](https://github.com/cscdpn/audiochaos)
as the instrument, all in Rust:
- a track is a Rust file that places stems and their score;
- the `studio` program composes it, renders each stem offline on its own
  audiochaos rack, then mixes, masters and analyzes the result.

Every sound comes out of audiochaos's modules. audiochaos is used as a
library and is not modified.

## Setup

- **audiochaos:** cargo fetches it from GitHub, pinned in `Cargo.toml` to
  the commit the studio is built and tested against.
- **ffmpeg:** by default, the `ffmpeg` program must be on the PATH. The
  `native` build option does without it (see [Without ffmpeg](#without-ffmpeg)).

## Quick start

```sh
cargo studio new demo      # writes tracks/demo/compose.rs
cargo studio build demo    # renders/demo/demo.mp3, among others
```

Tracks belong to their authors: git ignores `tracks/` and `renders/`, so
no track ever ends up in this repository.

## Commands

From this folder:

| Command | What it does |
|---|---|
| `cargo studio new <name>` | creates `tracks/<name>/compose.rs` from a template that already plays |
| `cargo studio build <name>` | composes, renders and masters into `renders/<name>/`: `<name>.mp3`, `<name>.wav` (24-bit, -12 LUFS), `mix.wav`, `stems/` |
| `cargo studio build <name> --bars 16` | renders only the first 16 bars, for a quick listen |
| `cargo studio analyze <name> [--zoom 40]` | gives the loudness of each stem per section, the clicks, the balance of the master and spectrograms |
| `cargo studio sound <patch.json or preset> --seconds 8` | renders a single patch: one sound, one texture |
| `cargo studio master <file.wav>` | masters a file |
| `cargo studio render <score.json>` | renders a score written by hand |

- **Options:** `cargo studio` on its own lists every option and the tracks.
- **Rebuilds:** `cargo studio` rebuilds whatever changed before running the
  command.
- **New tracks:** a new `tracks/<name>/` folder is picked up on its own,
  with nothing to declare.

## Writing a track

`compose.rs` defines `compose()`, which returns the track:

```rust
use audiochaos_studio::prelude::*;

pub fn compose() -> Track {
    let mut t = Track::new(128, 16);
    t.mark("intro", 0);
    t.mark("groove", 4);

    let mut kick = Kick::new(&t, "kick");
    kick.hits(t.steps("x... x... x... x...", 4, 16));

    let mut hats = Hat::new(&t, "hats").mix(-6.0, 0.3);
    hats.hits(t.steps(".4x4", 0, 16));

    let mut bass = Mono::new(&t, "bass").mix(-4.0, 0.0);
    bass.notes(&t.melody(". A1 A1 A2 . A1 C2! A1", 4, 16));

    t.add(kick);
    t.add(hats);
    t.add(bass);
    t
}
```

- **Time:** every time is in beats.
  - Bars and beats count from 0: `t.bar(16)` is the start of bar 16, and
    `t.at(16, 2)` is its third beat.
  - `t.ms(80)` converts milliseconds to beats.
- **Patterns:** one step is a sixteenth note.
  - The `_with` variants change the step length and add swing.
  - A pattern that does not fill whole bars runs across the bar lines,
    which makes polyrhythms.
  - The three kinds:
    - `t.steps("x.5.", …)` gives hits: `x` full, `1` to `9` softer, `.` a
      rest;
    - `t.gates("x--.", …)` gives notes held by `-`;
    - `t.melody("A1 - . C2!", …)` gives notes, one space-separated token
      per step: `-` holds the note, `!` accents it, `|` is ignored.
- **Helpers:**
  - `fade` (a crescendo or a decrescendo);
  - `euclid(3, 8, 0)` (Euclidean rhythms);
  - `hz`, `transpose` and `scale("A2", "phrygian", 1)`.
- **Sections:** `t.mark` names the sections. The analysis and the
  spectrograms use these names.
- **Mix order:** `t.add` adds a stem. Stems are mixed and listed in the
  order of the `add` calls.

## Instruments

Each instrument builds an audiochaos patch and knows how to play it.
- **Defaults:** every instrument plays as is, and together the defaults
  make a dark, industrial kit.
- **Changing them:** use `with` and a settings struct, as in
  `Hat::with(&t, "ohat", HatSound { decay_ms: 170.0, ..HatSound::default() })`.

| Instrument | Played with | Settings (`…Sound`) |
|---|---|---|
| `Kick` | `hit`, `hits`, `times()` | `tune`, `click`, `decay_ms`, `drive`, `lowpass` (to muffle it) |
| `Snare` | `hit`, `hits` | `tone`, `body`, `decay_ms`, `room_ms` |
| `Hat` | `hit`, `hits` | `cutoff`, `decay_ms` (40 closed, 170 open), `crush` |
| `Metal` (pickaxe, bell) | `hit`, `hits` | `pitch`, `ratio`, `index`, `noise`, `decay_ms` (90 pickaxe, 3000 bell) |
| `Mono` (bass, lead) | `note`, `notes`, `sweep` | `wave`, `detune_cents`, `filter`, `cutoff`, `peak`, `sustain`, `echo`, `crush` |
| `Chaos` | `stab`, `stabs`, knobs | `delay_s`, `lfo_rate`, `osc2` |
| `Drone` | `tune`, `slide`, `duck`, knobs | `notes`, `cutoff`, `delay_s`, `sway_hz` |
| `Hum` (electrical cable) | `pitch`, knobs | `mains`, `cutoff`, `drive`, `bits` |
| `Pump` | `stroke`, `breath` (the hiss without the thump) | `thump`, `hiss`, `room` |
| `Space` (the tail of a large hall) | `hit`, `hits`: strike it along with the drums it carries | `cutoff`, `grain`, `bloom_ms`, `decay_ms` |
| `Fx` | `riser`, `crash`, `boom`, `cut` | |

`hit` returns the hit, which can then be adjusted:
`kick.hit(t.bar(56), 1.0).decay_ms(1100.0).tune(45.0)`.

### Ready-made moves

- **Sweep:** `sweep(at, octaves, over, curve)` moves the whole filter of a
  `Mono` by that many octaves, on top of each note's opening. The rack's
  sequencer, all its steps at the same level, serves as a fixed voltage.
- **Muffling a kick:** with `lowpass: Some(18_000.0)`, bring
  `filter/cutoff` down towards 300 Hz, then open it again with a ramp.
- **Sliding between chords:** `slide` takes the two saws of a `Drone` to
  new pitches, at an even rate in semitones.
- **A reverb without echoes:** audiochaos has no reverb, and its delay line
  repeats what it receives. `Space` builds the tail of a hall out of noise
  instead:
  - the noise is sampled and held `grain` times per second, then
    band-passed;
  - it swells after each hit, then dies away while darkening by an octave;
  - two `Space`s with different `grain` values give two independent noises,
    for the left and the right. Without that, they would sound the same,
    since every rack uses the same noise seed.

### Playing by hand

Every instrument can also be played by hand, like a plain stem:
- `set(at, "module/knob", value)` turns a knob at once;
- `glide(at, "module/knob", value, over, curve)` turns it with a ramp.
  - A `curve` of 0 makes a straight line.
  - A positive `curve` moves fast, then slows down.
  - A negative `curve` starts slowly.
- `connect(at, "module/output -> module/input")` plugs a cable in;
- `disconnect(…)` unplugs it.

### Stereo

- **Pairs:** `Stereo::new(left, right)` plays two stems as one. Two
  slightly different copies give width.
- **Both sides:** `both(|side| …)` plays the rest on the pair, one side and
  then the other.
- **Cost:** adding stems costs almost nothing. A track of 1:45 with 10
  stems renders in about 7 seconds.

## A custom sound

- `Stem::new(&t, "name", patch)` takes a patch built with
  `Patcher::new("…").knob(…).cable(…).build()`.
- `Stem::preset(&t, "name", "chaos")` takes a built-in audiochaos preset.
- `Stem::file(&t, "name", "sounds/lead.json")` takes a patch exported from
  audiochaos's graphical editor (Patch menu, Export…). The path is relative
  to the track's folder. This is how to tune a sound by ear before using it
  in a track.

Everything `build` writes goes to `renders/<name>/`:
- the score (`track.json`);
- the patches;
- the stems;
- the mix and the master.

That folder is rewritten on every build.

## Without ffmpeg

By default, ffmpeg does three jobs:
- the mastering (limiter, and MP3 with LAME at 320 kbps);
- the loudness measurements;
- the spectrograms.

The `native` build option does all of it in Rust and needs no external
program:

```sh
cargo studio-native build <name>     # or: cargo build --release --features native
```

- **Measurements:** loudness and true peak come from the `ebur128` crate.
- **Limiter:** it works on true peaks (oversampled four times).
- **MP3:** it comes from `rusty_mp3`, at a constant 256 kbps.
- **Spectrograms:** they are drawn in Rust, with a line at the start of
  each section.

Measured on a track of 1:45 with 10 stems:
- **Master:** ffmpeg measures the same values on both masters (-12.0 LUFS,
  true peak -0.9 dBTP, loudness range 6.4 LU).
- **MP3:** at the same bitrate, the `rusty_mp3` file strays further from
  the source than the LAME one in the mids (5 to 9 dB more error), and it
  starts 22 ms late. LAME stays the reference.

## Good to know

- **Attacks:** audiochaos smooths every knob over 5 ms, so a sharp attack
  comes from plugging a cable in. The drums do it on their own
  (`Stem::strike`):
  1. the gain is set with the cable unplugged;
  2. the cable is plugged in;
  3. the gain falls back;
  4. the cable is unplugged as late as possible.
- **Timing:** changes land on 64-sample blocks, which is 1.3 ms at the
  48 kHz render rate.
- **DC offset:** audiochaos's distortions clip asymmetric waves, which
  leaves a DC offset. Like a mixing desk, the render cuts each stem below
  10 Hz.
- **Levels:** every rack ends with the `tanh` of its master module.
  - Past full level, a sound saturates instead of getting louder.
  - To bring a sound forward, use the mix gain (`mix`). If that is not
    enough for one section, give that section its own stem.
- **Echoes:** cutting an echo's feedback at once also cuts its tail, one
  trip around the delay line later. To end an echo cleanly, glide
  `delay/feedback` down, then `delay/mix`.
- **Rack limits:** a stem is one audiochaos rack of 13 modules, one of
  each, with two oscillators, in mono. For more modules, add stems. For
  stereo, pan them.
- **Score:** the format is described in [src/score.rs](src/score.rs). Its
  `meta` key (section markers, meter) is for the analysis; the render
  ignores it.
- **Library:** the studio is also a Rust library (`audiochaos_studio`) that
  other projects can use.

## Tests and checks

- **Tests:** `cargo test --release` for the default build, and
  `cargo test --release --features native` for the build without ffmpeg.
- **Lints:** `cargo clippy --release --all-targets`, with and without
  `--features native`.
- **Formatting:** `cargo fmt --check`.

## Windows with the GNU toolchain

- **Rust:** the GNU toolchain installs through `rustup` without admin
  rights or MSVC.
- **Build tools:** the `windows` crates then need `dlltool` and `as`, which
  `scoop install binutils` provides.
