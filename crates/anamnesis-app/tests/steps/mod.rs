//! Step definitions for the `.feature` files under `crates/anamnesis-app/features/`.
//! Split by feature for readability; every function auto-registers itself
//! with `cucumber` via its `#[given]`/`#[when]`/`#[then]` attribute, so the
//! grouping here is organisational only.

mod access_control;
mod archive_sweep;
mod bulk_add;
mod collaboration;
mod placement;
mod project_lifecycle;
mod relationships;
mod suggestions;
mod tangles;
mod task_lifecycle;
mod world;

pub use world::AppWorld;
