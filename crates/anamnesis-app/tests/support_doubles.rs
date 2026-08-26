//! Unit tests for the test doubles themselves — the parts of `support` that
//! neither the use-case tests nor the cucumber steps happen to exercise
//! (`BoardRepository::seed`, `SequentialIdGen::starting_at`, and the
//! `IdentityProvider` port's stub). Kept separate so `support` stays a
//! genuinely reusable double rather than dead code.

mod support;

use anamnesis_app::{BoardRepository, IdGen, IdentityProvider, LoginCallback};
use anamnesis_core::UserId;
use support::{InMemoryBoardRepository, SequentialIdGen, StubIdentityProvider};

fn some_board() -> anamnesis_app::Board {
    let ids = SequentialIdGen::new();
    anamnesis_core::create_board(
        anamnesis_core::BoardId::new(ids.next()),
        UserId::new("alice"),
        "Seeded Board",
    )
    .unwrap()
}

#[tokio::test]
async fn seed_makes_a_board_loadable_as_if_previously_saved() {
    let repo = InMemoryBoardRepository::new();
    let board = some_board();

    repo.seed(board.clone());

    assert_eq!(repo.load(board.id).await.unwrap(), Some(board));
}

#[test]
fn starting_at_hands_out_ids_from_the_given_offset() {
    let ids = SequentialIdGen::starting_at(100);

    assert_eq!(ids.next(), uuid::Uuid::from_u128(100));
    assert_eq!(ids.next(), uuid::Uuid::from_u128(101));
}

#[tokio::test]
async fn stub_identity_provider_completes_a_login_matching_its_own_redirect() {
    let provider = StubIdentityProvider::always_authenticating_as(UserId::new("alice"));

    let redirect = provider.begin_login().await.unwrap();
    let user = provider
        .complete_login(LoginCallback {
            code: "irrelevant-code".to_string(),
            state: redirect.csrf_state.clone(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await
        .unwrap();

    assert_eq!(user, UserId::new("alice"));
}

#[tokio::test]
async fn stub_identity_provider_rejects_a_callback_with_mismatched_state() {
    let provider = StubIdentityProvider::always_authenticating_as(UserId::new("alice"));
    let redirect = provider.begin_login().await.unwrap();

    let result = provider
        .complete_login(LoginCallback {
            code: "irrelevant-code".to_string(),
            state: "tampered-state".to_string(),
            expected_state: redirect.csrf_state.clone(),
            pkce_verifier: redirect.pkce_verifier.clone(),
            expected_nonce: redirect.nonce.clone(),
        })
        .await;

    assert!(result.is_err());
}
