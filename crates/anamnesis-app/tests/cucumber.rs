//! Cucumber/Gherkin BDD entry point. Wired as a `harness = false` `[[test]]`
//! target so `cargo test --workspace` runs it. Feature files live in
//! `crates/anamnesis-app/features/`; steps live in `tests/steps/`.

mod domain_fakes;
mod steps;
mod support;

use cucumber::World as _;

use steps::AppWorld;

#[tokio::main]
async fn main() {
    AppWorld::cucumber()
        .fail_on_skipped()
        .run_and_exit("features")
        .await;
}
