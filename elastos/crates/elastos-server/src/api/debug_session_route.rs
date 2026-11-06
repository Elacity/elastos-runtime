#[test]
fn server_source_does_not_mint_a_session_without_the_attach_secret() {
    let source = include_str!("server.rs");
    assert!(
        !source.contains("/api/test/create-session"),
        "a session mint without the attach secret must not be routed"
    );
    assert!(
        !source.contains("create_test_session"),
        "the unauthenticated session helper must not be mounted"
    );
}
