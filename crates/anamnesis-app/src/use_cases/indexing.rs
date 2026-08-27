//! Shared policy for keeping [`crate::ports::SearchIndex`] in step with a
//! write that already succeeded through a repository port.
//!
//! **Indexing runs beside the repository write, in the use case, not in
//! `anamnesis-web`'s handlers.** Handlers are transport: any caller of these
//! use cases — the web UI today, a future MCP server or CLI or JSON API per
//! `docs/CONTEXT.md`'s "no MCP just yet" — must get a consistent index
//! without having to remember to update it itself. This module exists so
//! every use case that touches an indexable entity (area/project/task)
//! shares one place to decide what happens when the index write itself
//! fails.
//!
//! **Decision: an index-write failure is logged and non-fatal.** The search
//! index is derived, rebuildable data (`docs/DOMAIN.md` §7's `SearchIndex`
//! doc comment; it exists purely to keep `SearchQuery` current). By the time
//! [`log_index_failure`] is called, the entity's own repository write has
//! already committed — the user's create/edit/archive succeeded and must not
//! be rolled back or reported as a failure just because the *index* fell out
//! of step. So a use case calls the index port, and on `Err` logs and
//! continues rather than propagating an `AppError` that would tell the
//! caller their write failed when it didn't. The cost is an accepted,
//! documented one: a title can transiently go missing (or stale) from search
//! results until the next successful write to that entity, or a future
//! reindex sweep. `anamnesis-app` depends on nothing but `anamnesis-core`,
//! `async-trait`, and value crates (no logging crate among them), so
//! `eprintln!` — plain `std`, not a dependency — is the only channel
//! available at this layer; `anamnesis-web`'s adapters have `tracing` for
//! anything richer.
pub(crate) fn log_index_failure(operation: &str, err: crate::error::RepoError) {
    eprintln!("anamnesis: search index update failed during {operation}: {err}");
}
