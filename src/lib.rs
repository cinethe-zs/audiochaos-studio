//! audiochaos-studio: music made with audiochaos as the instrument.
//!
//! A track is Rust code (`tracks/<name>/compose.rs`) that places stems,
//! each one an audiochaos rack with a patch, and the knob moves and cable
//! changes that play them. The `studio` program composes, renders on
//! audiochaos's own engine, masters and analyzes.

pub mod analyze;
pub mod compose;
pub mod dsp;
pub mod instruments;
pub mod media;
pub mod score;
pub mod wav;

/// Everything a track's compose.rs uses.
pub mod prelude {
    pub use crate::compose::{
        Clock, Note, Part, Patcher, Pitch, Stem, Stereo, Track, Voice, euclid, fade, hz, midi,
        note_name, scale, transpose,
    };
    pub use crate::instruments::{
        Chaos, ChaosSound, Drone, DroneSound, Echo, Filter, Fx, Hat, HatSound, Hit, Hum, HumSound,
        Kick, KickSound, Metal, MetalSound, Mono, MonoSound, Pump, PumpSound, Snare, SnareSound,
        Space, SpaceSound, Wave,
    };
}
