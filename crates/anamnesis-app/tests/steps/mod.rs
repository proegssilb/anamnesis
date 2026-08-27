//! Step definitions for the `.feature` files under `crates/anamnesis-app/features/`.
//! Split by feature for readability; every function auto-registers itself
//! with `cucumber` via its `#[given]`/`#[when]`/`#[then]` attribute, so the
//! grouping here is organisational only.

mod access_control;
mod placement;
mod suggestions;
mod tangles;
mod world;

pub use world::AppWorld;
