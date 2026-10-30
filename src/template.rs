//! NAME: a new track for audiochaos-studio.
//!
//!     cargo studio build NAME

use audiochaos_studio::prelude::*;

pub fn compose() -> Track {
    let mut t = Track::new(128, 16);
    t.mark("intro", 0);
    t.mark("groove", 4);
    t.mark("end", 15);

    let mut kick = Kick::new(&t, "kick");
    kick.hits(t.steps("x... x... x... x...", 4, 15));
    kick.hit(t.bar(15), 1.0).decay_ms(900.0).tune(45.0);
    let kicks: Vec<f64> = kick
        .times()
        .into_iter()
        .filter(|&k| k < t.bar(15))
        .collect();

    let mut hats = Hat::new(&t, "hats").mix(-6.0, 0.3);
    hats.hits(t.steps(".4x4", 0, 15));

    let mut snare = Snare::new(&t, "snare").mix(-1.0, 0.0);
    snare.hits(t.steps("....x... ....x...", 8, 15));

    let mut bass = Mono::new(&t, "bass").mix(-4.0, 0.0);
    for note in t.melody(". A1 A1 A2 . A1 C2 A1 . A1 A1 G1 . A1 E2! A1", 4, 15) {
        // The first note after each kick bites harder, and so do the marked ones.
        bass.note(
            note.at,
            note.pitch.as_str(),
            t.ms(80),
            note.accent || note.at % 1.0 == 0.25,
        );
    }

    let sound = DroneSound {
        notes: (hz("A1"), hz("E2")),
        ..DroneSound::default()
    };
    let mut drone = Drone::with(&t, "drone", sound).mix(-6.0, 0.0);
    drone.glide(0.0, "vca/gain", 0.6, t.bar(2), 0.0);
    drone.duck(&kicks, |_| 0.35);
    drone.glide(t.bar(15), "vca/gain", 0.0, 4.0, 0.0);

    t.add(kick);
    t.add(hats);
    t.add(snare);
    t.add(bass);
    t.add(drone);
    t
}
