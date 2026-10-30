//! The tracks, compiled in from tracks/<name>/compose.rs by build.rs.

/// A track's `compose` function.
pub type Compose = fn() -> audiochaos_studio::compose::Track;

include!(concat!(env!("OUT_DIR"), "/tracks.rs"));

pub fn find(name: &str) -> Option<Compose> {
    TRACKS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|&(_, compose)| compose)
}
