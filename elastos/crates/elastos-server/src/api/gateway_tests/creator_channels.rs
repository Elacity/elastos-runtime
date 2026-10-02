use super::*;

/// The directory answers the question this surface is actually asking.
///
/// `access: "mint:0x…"` is the set the account may PUBLISH into. The wider
/// `user:` prefix also answers, with channels the account can reach but not
/// necessarily publish into -- offering one of those would put a channel in
/// the picker that refuses the mint. Checked live against the Base directory
/// while this was written: `mint:` returned two channels where `user:`
/// returned three.
///
/// Pinned as a test because a wrong filter does not fail. It returns a
/// plausible list of the wrong channels, which is the kind of mistake nobody
/// notices until a mint is refused.
#[test]
fn test_creator_channel_directory_asks_for_channels_the_account_can_mint_into() {
    assert!(CREATOR_CHANNELS_QUERY.contains("fetchChannels"));
    assert!(CREATOR_CHANNELS_QUERY.contains("$query: ChannelQueryInput"));
    let creator = "0x4a62316623ad457f02cdc5d997ded67a383ec569";
    assert_eq!(
        creator_channels_access_filter(creator),
        "mint:0x4a62316623ad457f02cdc5d997ded67a383ec569"
    );
    assert!(!creator_channels_access_filter(creator).starts_with("user:"));
    // One page is the whole picker, and it is the page size the product's own
    // channel list asks for.
    assert_eq!(CREATOR_CHANNELS_PAGE_LIMIT, 50);
}

/// A refusal must not read as "this creator has no channels".
///
/// GraphQL reports failures inside a 200 response, so a directory that refused
/// the query would otherwise parse as an empty list and the picker would
/// quietly offer nothing rather than say what happened.
#[test]
fn test_creator_channel_directory_errors_are_not_an_empty_list() {
    let refused = json!({
        "errors": [{ "message": "Field \"access\" is not defined by type \"ChannelQueryInput\"." }],
        "data": { "result": null }
    });
    let err = creator_channels_from_directory_payload(&refused).unwrap_err();
    assert!(err.to_string().contains("not defined by type"), "{err}");

    let answered = json!({
        "data": { "result": { "total": 2, "data": [
            { "address": "0x0EBAC909D31EF0074495E752C0CF4EA49BA13C41", "name": "Test CH-3.0-rc7" },
            { "address": "0x56d2d76a8e3a1c9551efe1201e15b517f77cb9b0", "name": "Non-Media Sandbox" }
        ] } }
    });
    let channels = creator_channels_from_directory_payload(&answered).unwrap();
    assert_eq!(channels.len(), 2);
    // Addresses are compared and stored lowercase, so a directory that spells
    // one differently still matches the configured channel.
    assert_eq!(
        channels[0].address,
        "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41"
    );
    assert_eq!(channels[1].name, "Non-Media Sandbox");

    // A row this Runtime cannot address is not a channel it can offer.
    let malformed = json!({
        "data": { "result": { "data": [
            { "address": "not-an-address", "name": "Nope" },
            { "name": "No address at all" }
        ] } }
    });
    assert!(creator_channels_from_directory_payload(&malformed)
        .unwrap()
        .is_empty());
}

/// External HTTP stays off until someone says otherwise, and says it for THIS
/// directory: approving the Wallet's price source must not approve this one.
#[test]
fn test_creator_channel_directory_requires_its_own_explicit_approval() {
    // Nothing configured at all.
    assert!(CREATOR_CHANNEL_DIRECTORY
        .source_decision(|_| None, || None)
        .is_err());
    // Source named, approval absent.
    assert!(CREATOR_CHANNEL_DIRECTORY
        .source_decision(
            |name| match name {
                "ELASTOS_CREATOR_CHANNELS_SOURCE" => Some("onchain_graphql".to_string()),
                _ => None,
            },
            || None,
        )
        .is_err());
    // Source named and approved by env.
    assert!(CREATOR_CHANNEL_DIRECTORY
        .source_decision(
            |name| match name {
                "ELASTOS_CREATOR_CHANNELS_SOURCE" => Some("onchain_graphql".to_string()),
                "ELASTOS_CREATOR_CHANNELS_HTTP_APPROVED" => Some("true".to_string()),
                _ => None,
            },
            || None,
        )
        .is_ok());
    // Approved by a recorded policy, which is what the Inbox action writes.
    assert!(CREATOR_CHANNEL_DIRECTORY
        .source_decision(
            |_| None,
            || Some(OnchainDirectoryPolicy {
                schema: "elastos.creator.channel-directory-policy/v1".to_string(),
                source: "onchain_graphql".to_string(),
                external_http_approved: true,
                approved_by_principal_id: "did:example:someone".to_string(),
                approved_at: 1_731_000_000,
            }),
        )
        .is_ok());
    // A policy for some other directory approves nothing here.
    assert!(CREATOR_CHANNEL_DIRECTORY
        .source_decision(
            |_| None,
            || Some(OnchainDirectoryPolicy {
                schema: "elastos.creator.channel-directory-policy/v1".to_string(),
                source: "somewhere-else".to_string(),
                external_http_approved: true,
                approved_by_principal_id: "did:example:someone".to_string(),
                approved_at: 1_731_000_000,
            }),
        )
        .is_err());
}

/// Only a refusal the person can resolve should raise an approval request; a
/// directory that is simply down must not put an Inbox item in front of them.
#[test]
fn test_creator_channel_directory_only_asks_when_asking_would_help() {
    assert!(CREATOR_CHANNEL_DIRECTORY.note_should_request_approval(
        "channel directory source is not configured; approve an external HTTP source to list channels"
    ));
    assert!(CREATOR_CHANNEL_DIRECTORY
        .note_should_request_approval("external channel directory HTTP source is not approved"));
    assert!(!CREATOR_CHANNEL_DIRECTORY
        .note_should_request_approval("channel directory returned 502: bad gateway"));
    assert!(!CREATOR_CHANNEL_DIRECTORY.note_should_request_approval(
        "channel directory error: Field \"access\" is not defined by type \"ChannelQueryInput\"."
    ));
}
