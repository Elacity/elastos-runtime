use super::*;

/// Shops asks a different question from the Creator's picker, and the
/// difference matters: `access: "mint:0x…"` is the set THIS account may
/// publish into, which as a shop window would show a person their own
/// permissions rather than the market. Shops asks for the channels that exist.
///
/// Pinned because the wrong filter does not fail. It answers with a plausible
/// list of the wrong channels.
#[test]
fn test_market_directory_lists_channels_rather_than_this_account_s_own() {
    assert!(
        !MARKET_CHANNELS_QUERY.contains("access"),
        "Shops must not filter channels by what this account may mint into: {MARKET_CHANNELS_QUERY}"
    );
    assert!(MARKET_CHANNELS_QUERY.contains("fetchChannels"));
    // What a shop card shows. `itemsCount` is the index's arithmetic over
    // chain state -- fine on a card, never a term of a sale.
    for field in ["address", "name", "description", "categories", "itemsCount"] {
        assert!(
            MARKET_CHANNELS_QUERY.contains(field),
            "Shops cards need {field}"
        );
    }
}

/// A refused query is not an empty market, and a row without a usable address
/// is not a channel this Home can show.
#[test]
fn test_market_directory_errors_are_not_an_empty_market() {
    let refused = json!({
        "errors": [{ "message": "Field \"itemsCount\" is not defined by type \"Channel\"." }],
        "data": { "result": null }
    });
    let err = market_channels_from_directory_payload(&refused).unwrap_err();
    assert!(err.to_string().contains("not defined by type"), "{err}");

    let answered = json!({
        "data": { "result": { "total": 2, "data": [
            {
                "address": "0x0EBAC909D31EF0074495E752C0CF4EA49BA13C41",
                "name": "Test CH-3.0-rc7",
                "description": "A channel",
                "categories": ["music", "video"],
                "itemsCount": 40
            },
            { "address": "0x56d2d76a8e3a1c9551efe1201e15b517f77cb9b0", "name": "Non-Media Sandbox" }
        ] } }
    });
    let channels = market_channels_from_directory_payload(&answered).unwrap();
    assert_eq!(channels.len(), 2);
    // Addresses are stored lowercase, so a directory that spells one
    // differently still matches what this Home holds.
    assert_eq!(
        channels[0].address,
        "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41"
    );
    assert_eq!(channels[0].items_count, 40);
    assert_eq!(channels[0].categories, vec!["music", "video"]);
    // Absent fields are absent, not invented.
    assert_eq!(channels[1].description, "");
    assert_eq!(channels[1].items_count, 0);

    let malformed = json!({
        "data": { "result": { "data": [
            { "address": "not-an-address", "name": "Nope" },
            { "name": "No address at all" }
        ] } }
    });
    assert!(market_channels_from_directory_payload(&malformed)
        .unwrap()
        .is_empty());
}

/// A directory is free to answer with a megabyte of prose. The half that
/// decided to ask is the half that refuses it, so a card cannot be handed
/// something no card can hold.
#[test]
fn test_market_directory_bounds_what_the_index_says() {
    let long = "x".repeat(4096);
    let answered = json!({
        "data": { "result": { "data": [{
            "address": "0x56d2d76a8e3a1c9551efe1201e15b517f77cb9b0",
            "name": long,
            "description": long,
            "categories": (0..40).map(|index| format!("c{index}")).collect::<Vec<_>>(),
        }] } }
    });
    let channels = market_channels_from_directory_payload(&answered).unwrap();
    assert_eq!(channels[0].name.chars().count(), 256);
    assert_eq!(channels[0].description.chars().count(), 256);
    assert_eq!(channels[0].categories.len(), 8);
}

/// Marketplace's approval is its own. The Creator's channel-list approval must
/// not enable it, and its own must not enable the Creator's -- one endpoint is
/// not one consent, and the two surfaces ask for different things.
#[test]
fn test_market_directory_is_not_approved_by_the_creator_s_approval() {
    let creator_policy = || {
        Some(OnchainDirectoryPolicy {
            schema: "elastos.creator.channel-directory-policy/v1".to_string(),
            source: "onchain_graphql".to_string(),
            external_http_approved: true,
            approved_by_principal_id: "did:example:someone".to_string(),
            approved_at: 1_731_000_000,
        })
    };
    let market_policy = || {
        Some(OnchainDirectoryPolicy {
            schema: "elastos.marketplace.directory-policy/v1".to_string(),
            source: "onchain_graphql".to_string(),
            external_http_approved: true,
            approved_by_principal_id: "did:example:someone".to_string(),
            approved_at: 1_731_000_000,
        })
    };
    // `load_policy` is what refuses a file written for another surface, so the
    // schemas are asserted to differ rather than the decision being asked to
    // tell them apart after the fact.
    assert_ne!(
        CREATOR_CHANNEL_DIRECTORY.policy_schema,
        MARKET_DIRECTORY.policy_schema
    );
    assert_ne!(
        CREATOR_CHANNEL_DIRECTORY.policy_file,
        MARKET_DIRECTORY.policy_file
    );
    assert_ne!(
        CREATOR_CHANNEL_DIRECTORY.approve_action_id,
        MARKET_DIRECTORY.approve_action_id
    );
    assert_ne!(
        CREATOR_CHANNEL_DIRECTORY.request_id,
        MARKET_DIRECTORY.request_id
    );
    // Neither surface reads the other's environment variables either.
    assert_ne!(
        CREATOR_CHANNEL_DIRECTORY.approved_env,
        MARKET_DIRECTORY.approved_env
    );
    assert!(MARKET_DIRECTORY
        .source_decision(|_| None, creator_policy)
        .is_ok());
    assert!(CREATOR_CHANNEL_DIRECTORY
        .source_decision(|_| None, market_policy)
        .is_ok());
    // Nothing configured approves nothing.
    assert!(MARKET_DIRECTORY.source_decision(|_| None, || None).is_err());
}

/// Only a refusal the person can resolve raises an Inbox item. A directory
/// that is down must not ask anyone for permission it already has.
#[test]
fn test_market_directory_only_asks_when_asking_would_help() {
    assert!(MARKET_DIRECTORY.note_should_request_approval(
        "market directory source is not configured; approve an external HTTP source to list channels"
    ));
    assert!(MARKET_DIRECTORY
        .note_should_request_approval("external market directory HTTP source is not approved"));
    assert!(!MARKET_DIRECTORY
        .note_should_request_approval("market directory returned 502: bad gateway"));
    // And one surface's refusal is not the other's to answer.
    assert!(!CREATOR_CHANNEL_DIRECTORY
        .note_should_request_approval("external market directory HTTP source is not approved"));
}

/// The deny action of one directory must not dismiss the other's request.
#[test]
fn test_market_directory_deny_action_is_its_own() {
    assert!(MARKET_DIRECTORY.is_deny_action("marketplace-directory-http-deny:onchain-graphql"));
    assert!(!MARKET_DIRECTORY.is_deny_action("creator-channels-http-deny:onchain-graphql"));
    assert!(!CREATOR_CHANNEL_DIRECTORY
        .is_deny_action("marketplace-directory-http-deny:onchain-graphql"));
    // The Inbox renders Approve/Reject from the `-http-approve:` spelling, so
    // a new directory that misspells it gets no buttons at all.
    assert!(MARKET_DIRECTORY
        .approve_action_id
        .contains("-http-approve:"));
}

/// The index's URL fields are not URLs, and one of them points into a private
/// network. Only a CID is taken, because a CID is something this Home can
/// fetch for itself.
///
/// Every string below is from a live answer of the configured directory.
#[test]
fn test_market_directory_takes_only_a_cid_for_a_channel_picture() {
    let answered = json!({
        "data": { "result": { "data": [
            // A real cover: a bare CIDv0.
            {
                "address": "0x948d0561c6111046bca46ba20b8c04b20fdbd2e3",
                "name": "REV 1",
                "image": "",
                "imageURL": null,
                "coverImage": "QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH",
                "coverImageURL": "thumbnail:1789921246918.jpg"
            },
            // `initials:W` is the index saying "draw the letter", and its
            // imageURL is their internal gateway on an RFC1918 address. A Home
            // that followed it would be fetching from a private network.
            {
                "address": "0x1431cf1924654df027d9d7184a9e721b42a10ce4",
                "name": "wow",
                "image": "initials:W",
                "imageURL": "http://10.132.0.5:8080/ipfs/initials:W"
            },
            // No picture at all, which is the common case.
            { "address": "0x472244761299916b1a5271fbb070b49632ef078a", "name": "AFF", "image": "" }
        ] } }
    });
    let channels = market_channels_from_directory_payload(&answered).unwrap();
    assert_eq!(channels.len(), 3);
    assert_eq!(
        channels[0].image_cid,
        "QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH"
    );
    // The sentinel is not a CID, and neither is the private-network URL that
    // came with it. This channel keeps its glyph.
    assert_eq!(channels[1].image_cid, "");
    assert_eq!(channels[2].image_cid, "");

    // Nothing that is not a CID may reach the fetch, whatever it looks like.
    for refused in [
        "",
        "initials:W",
        "thumbnail:1789921246918.jpg",
        "http://10.132.0.5:8080/ipfs/QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH",
        "https://example.invalid/image.png",
        "ipfs://QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH",
        "QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH/../../etc/passwd",
        "../../etc/passwd",
        "Qm0OIl",
        "Qmshort",
    ] {
        let payload = json!({
            "data": { "result": { "data": [
                { "address": "0x472244761299916b1a5271fbb070b49632ef078a", "image": refused }
            ] } }
        });
        let channels = market_channels_from_directory_payload(&payload).unwrap();
        assert!(
            channels[0].image_cid.is_empty(),
            "a channel picture must not be fetched from {refused:?}"
        );
    }

    // Both CID spellings are accepted.
    for accepted in [
        "QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH",
        "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
    ] {
        let payload = json!({
            "data": { "result": { "data": [
                { "address": "0x472244761299916b1a5271fbb070b49632ef078a", "image": accepted }
            ] } }
        });
        let channels = market_channels_from_directory_payload(&payload).unwrap();
        assert_eq!(channels[0].image_cid, accepted);
    }
}

/// A CID is what reaches the page, and only a CID.
///
/// This reverses an earlier decision. The first version resolved pictures by
/// channel so that a capsule could never name what this Home fetched. That
/// meant a second content path existing for one surface, while `/ipfs/:cid`
/// already fetches CID content from the local node and caches it -- and
/// Explore's covers would have needed the same thing again.
///
/// So the page is handed the CID and asks that route for it. What matters is
/// what never reaches the page: the index's own URL fields, which point at a
/// private address and at someone else's thumbnail service.
#[test]
fn test_market_directory_hands_the_page_a_cid_and_never_a_url() {
    let answered = json!({
        "data": { "result": { "data": [{
            "address": "0x948d0561c6111046bca46ba20b8c04b20fdbd2e3",
            "name": "REV 1",
            "image": "initials:R",
            "imageURL": "http://10.132.0.5:8080/ipfs/initials:R",
            "coverImage": "QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH",
            "coverImageURL": "thumbnail:1789921246918.jpg"
        }] } }
    });
    let channels = market_channels_from_directory_payload(&answered).unwrap();
    let serialized = serde_json::to_string(&channels[0]).unwrap();
    assert!(
        serialized.contains("\"imageCid\":\"QmYtisnG1wCaUCAGp3hi2xKKeaTzMFhvxyFf1crWaTyoRH\""),
        "{serialized}"
    );
    assert!(
        !serialized.contains("10.132.0.5")
            && !serialized.contains("thumbnail:")
            && !serialized.contains("initials:"),
        "no directory URL or sentinel may reach the page: {serialized}"
    );
}

/// The index answers in its own dialect, and every value is converted before
/// a capsule sees it.
///
/// The row below is a real answer from the configured index, unedited. Each
/// difference it carries is one a surface would otherwise have to know about.
#[test]
fn test_market_catalog_converts_the_index_dialect() {
    let payload = json!({
        "data": { "result": { "total": 116, "data": [{
            "__typename": "ProtectedAsset",
            "contractAddress": "0x1431CF1924654df027d9d7184a9e721b42a10ce4",
            "hexTokenID": "0x0ba7651293c700df2c365f38e2c11a657378c9dce9d4481df9f81b88cfab2946",
            "name": "Testing",
            "contentType": null,
            "tokenURI": "ipfs://Qmci4hRgVsUg2oN254FrnG1TBCPXwbkHc2v3KKDUHSxXYi/metadata.json",
            "price": 0.1,
            "paymentToken": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            "priceInUSD": null,
            "views": 7,
            "createdAt": 1_790_138_251_000_u64,
            "metadata": {
                "kid": "27df70c564295e2fbc1967719ffdb12e",
                "media": { "uri": "ipfs://Qma6zsq5rQXK1dwtF6xvFSGNvVY8q5yGGUugH3bVTst5m8", "contentType": "video" }
            },
            "operative": {
                "address": "0x1aa4dd2cb9c9b784eaccedf5c679fd267cab1033",
                "opType": 2,
                "contentId": "27df70c564295e2fbc1967719ffdb12e",
                "access": { "listings": [] }
            }
        }] } }
    });
    let usdc = vec![(
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
        6u8,
    )];
    let items = market_catalog_from_payload(&payload, &usdc).unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    // Addresses are lowercased, whatever case the index used.
    assert_eq!(item.ledger, "0x1431cf1924654df027d9d7184a9e721b42a10ce4");
    // A price in token units becomes base units, exactly: 0.1 USDC is 100000.
    assert_eq!(item.price, "100000");
    // Milliseconds become seconds.
    assert_eq!(item.published_at, 1_790_138_251);
    // The content access id gains the `0x` this Home writes it with.
    assert_eq!(item.content_access_id, "0x27df70c564295e2fbc1967719ffdb12e");
    // The token URI names a document; what this Home fetches is its directory.
    assert_eq!(
        item.metadata_cid,
        "Qmci4hRgVsUg2oN254FrnG1TBCPXwbkHc2v3KKDUHSxXYi"
    );
    assert_eq!(
        item.content_cid,
        "Qma6zsq5rQXK1dwtF6xvFSGNvVY8q5yGGUugH3bVTst5m8"
    );
    assert_eq!(item.content_category, "video");
    assert_eq!(item.views, 7);
    assert_eq!(item.op_type, 2);
}

/// A price whose scale this Home cannot know is not shown as a price.
#[test]
fn test_market_catalog_refuses_a_price_it_cannot_scale() {
    let row = |price: serde_json::Value, token: &str| {
        json!({ "data": { "result": { "data": [{
            "contractAddress": "0x1431cf1924654df027d9d7184a9e721b42a10ce4",
            "hexTokenID": "0x7",
            "paymentToken": token,
            "price": price,
            "metadata": { "kid": "27df70c564295e2fbc1967719ffdb12e" }
        }] } } })
    };
    let usdc = vec![(
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
        6u8,
    )];
    let usdc_address = "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913";

    // A token missing from this Home's own list has unknown decimals, so its
    // price is not converted into a number that would look authoritative.
    let unknown = market_catalog_from_payload(
        &row(json!(0.1), "0x1111111111111111111111111111111111111111"),
        &usdc,
    )
    .unwrap();
    assert_eq!(unknown[0].price, "0");

    // More precision than the token has is refused rather than rounded:
    // rounding a price is changing it.
    let too_precise =
        market_catalog_from_payload(&row(json!(0.123_456_789), usdc_address), &usdc).unwrap();
    assert_eq!(too_precise[0].price, "0");

    // Whole numbers, strings and zero all convert exactly.
    for (given, expected) in [
        (json!(1), "1000000"),
        (json!("2.5"), "2500000"),
        (json!(0.000001), "1"),
        (json!(0), "0"),
    ] {
        let items = market_catalog_from_payload(&row(given.clone(), usdc_address), &usdc).unwrap();
        assert_eq!(items[0].price, expected, "price {given}");
    }
}

/// A row this Home could not act on is dropped rather than drawn as a card
/// whose buttons cannot do what they say. So is one the index has withdrawn.
#[test]
fn test_market_catalog_drops_what_it_cannot_use() {
    let usdc = vec![(
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
        6u8,
    )];
    let payload = json!({ "data": { "result": { "data": [
        // No ledger.
        { "hexTokenID": "0x7", "metadata": { "kid": "27df70c564295e2fbc1967719ffdb12e" } },
        // No token.
        { "contractAddress": "0x1431cf1924654df027d9d7184a9e721b42a10ce4",
          "metadata": { "kid": "27df70c564295e2fbc1967719ffdb12e" } },
        // No content id.
        { "contractAddress": "0x1431cf1924654df027d9d7184a9e721b42a10ce4", "hexTokenID": "0x7" },
        // Withdrawn by the index.
        { "contractAddress": "0x1431cf1924654df027d9d7184a9e721b42a10ce4", "hexTokenID": "0x8",
          "unpublished": true, "metadata": { "kid": "27df70c564295e2fbc1967719ffdb12e" } },
        // Usable.
        { "contractAddress": "0x1431cf1924654df027d9d7184a9e721b42a10ce4", "hexTokenID": "0x9",
          "metadata": { "kid": "27df70c564295e2fbc1967719ffdb12e" } }
    ] } } });
    let items = market_catalog_from_payload(&payload, &usdc).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].token_id, "0x9");
}

/// The index refuses a query without `type` and `filterby` -- with a 422, not
/// an empty answer -- so both are sent, and `variant` narrows it to the
/// assets this Home can do anything with.
#[test]
fn test_market_catalog_query_satisfies_the_index_validator() {
    let input = market_catalog_query_input();
    assert_eq!(input["type"], "single");
    assert_eq!(input["variant"], "drm");
    assert!(input["filterby"].is_array());
    assert!(MARKET_CATALOG_QUERY.contains("fetchNFTItems"));
    for field in [
        "hexTokenID",
        "tokenURI",
        "kid",
        "opType",
        "views",
        "createdAt",
    ] {
        assert!(
            MARKET_CATALOG_QUERY.contains(field),
            "catalog needs {field}"
        );
    }
}

/// Exponent notation is written out rather than multiplied, so no step of a
/// price passes through a float.
#[test]
fn test_market_catalog_expands_exponent_prices() {
    let usdc = vec![(
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
        6u8,
    )];
    let row = |price: &str| {
        json!({ "data": { "result": { "data": [{
            "contractAddress": "0x1431cf1924654df027d9d7184a9e721b42a10ce4",
            "hexTokenID": "0x7",
            "paymentToken": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            "price": price,
            "metadata": { "kid": "27df70c564295e2fbc1967719ffdb12e" }
        }] } } })
    };
    for (given, expected) in [
        ("1e-6", "1"),
        ("1E-6", "1"),
        ("1.5e-5", "15"),
        ("2e1", "20000000"),
        ("1.5e3", "1500000000"),
        // Beyond the token's precision, and so refused rather than rounded.
        ("1e-9", "0"),
    ] {
        let items = market_catalog_from_payload(&row(given), &usdc).unwrap();
        assert_eq!(items[0].price, expected, "price {given}");
    }
}

/// An item is something to buy until this Home holds a listing for it, and
/// the join that decides is made on the names the chain uses.
#[test]
fn test_market_catalog_item_is_buyable_until_this_home_holds_it() {
    let usdc = vec![(
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
        6u8,
    )];
    let payload = json!({ "data": { "result": { "data": [{
        "contractAddress": "0x1431cf1924654df027d9d7184a9e721b42a10ce4",
        "hexTokenID": "0x9",
        "metadata": { "kid": "27df70c564295e2fbc1967719ffdb12e" }
    }] } } });
    let item = &market_catalog_from_payload(&payload, &usdc).unwrap()[0];
    assert_eq!(item.access_state, "available");
    // No listing of this Home's own, so nothing to open and nothing to name.
    assert!(item.mint_id.is_empty());
    // And the mint id stays out of the answer entirely rather than arriving
    // as an empty string a surface would have to test for.
    let serialized = serde_json::to_string(item).unwrap();
    assert!(!serialized.contains("mintId"), "{serialized}");
    assert!(
        serialized.contains("\"accessState\":\"available\""),
        "{serialized}"
    );
}

/// Rows and errors arrive together, and the rows are still rows.
///
/// The live index answers a sixty-item page with sixty assets and eighteen
/// complaints -- `Cannot return null for non-nullable field
/// LedgerTokenMetadata.image` -- one per asset whose metadata has no image.
/// Reading `errors` first threw away every good row with the bad ones, and
/// Explore showed nothing while the index was answering perfectly well.
#[test]
fn test_directory_keeps_rows_that_arrived_with_complaints() {
    let payload = json!({
        "errors": [
            { "message": "Cannot return null for non-nullable field LedgerTokenMetadata.image.",
              "path": ["result", "data", 1, "metadata", "image"] }
        ],
        "data": { "result": { "total": 2, "data": [
            { "address": "0x0ebac909d31ef0074495e752c0cf4ea49ba13c41", "name": "Answered" },
            { "address": "0x56d2d76a8e3a1c9551efe1201e15b517f77cb9b0", "name": "Complained about" }
        ] } }
    });
    let channels = market_channels_from_directory_payload(&payload).unwrap();
    assert_eq!(
        channels.len(),
        2,
        "a complaint about one row is not an answer about the rest"
    );

    // A refusal is still a refusal: no rows at all, and the reason is kept.
    let refused = json!({
        "errors": [{ "message": "Field \"access\" is not defined by type \"ChannelQueryInput\"." }],
        "data": { "result": null }
    });
    let err = market_channels_from_directory_payload(&refused).unwrap_err();
    assert!(err.to_string().contains("not defined by type"), "{err}");

    // And an answer that is neither rows nor errors is neither.
    let empty = json!({ "data": { "result": null } });
    assert!(market_channels_from_directory_payload(&empty).is_err());
}

/// How a GraphQL answer is read is decided in one place, for every surface
/// that asks this index anything.
///
/// The rule it holds: when `data` carries rows, those rows are used even if
/// `errors` came with them, and the complaints go to the log rather than to a
/// person or to nowhere. A surface that read the payload itself would be free
/// to get that wrong again -- which cost an afternoon when it happened once,
/// because the shelf was empty while the index was answering perfectly well.
#[test]
fn test_only_one_place_reads_a_graphql_answer() {
    let api = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api");
    let mut readers = Vec::new();
    let mut askers = Vec::new();
    for entry in std::fs::read_dir(&api).expect("api sources") {
        let path = entry.expect("api entry").path();
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        if source.contains(r#"get("errors")"#) {
            readers.push(name.clone());
        }
        if source.contains(".post_graphql(") {
            askers.push(name);
        }
    }
    readers.sort();
    assert_eq!(
        readers,
        vec!["gateway_onchain_directory.rs".to_string()],
        "a GraphQL answer is read in one place; these read it themselves: {readers:?}"
    );
    // And everyone who asks goes through that place to read the answer.
    assert!(
        !askers.is_empty(),
        "no surface asks the index anything, which cannot be right"
    );
    let shared = std::fs::read_to_string(api.join("gateway_onchain_directory.rs")).unwrap();
    assert!(
        shared.contains("directory answered with rows and complaints; the rows are used"),
        "the shared reader must keep the rows and log the complaints"
    );
}

/// The index states a price in two different scales, and they must not be
/// treated as one.
///
/// Live rows, unedited: an item's headline `price` is in token units while
/// its listing's `price` is already in base units. Scaling both multiplied
/// every listed asset by a million and put `$200000` on a twenty-cent card.
#[test]
fn test_market_catalog_reads_each_price_in_its_own_scale() {
    let usdc = vec![(
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string(),
        6u8,
    )];
    let row = |listings: serde_json::Value, headline: serde_json::Value| {
        json!({ "data": { "result": { "data": [{
            "contractAddress": "0x1431cf1924654df027d9d7184a9e721b42a10ce4",
            "hexTokenID": "0x7",
            "paymentToken": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
            "price": headline,
            "metadata": { "kid": "27df70c564295e2fbc1967719ffdb12e" },
            "operative": { "access": { "listings": listings } }
        }] } } })
    };
    // A listing's price is base units already: 0.2 USDC is 200000, and 200000
    // is what must come out -- not 200000000000.
    let listed = json!([{ "price": 200000, "quantity": 10, "payToken": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913" }]);
    let items = market_catalog_from_payload(&row(listed, json!(0.2)), &usdc).unwrap();
    assert_eq!(items[0].price, "200000");
    assert_eq!(items[0].quantity, 10);

    // With no listing, the headline price is in token units and is scaled.
    let items = market_catalog_from_payload(&row(json!([]), json!(0.2)), &usdc).unwrap();
    assert_eq!(items[0].price, "200000");

    // A listing price with a fraction is not base units, whatever it claims.
    // It is not read at the wrong scale; the item's headline price answers
    // instead, which is in a scale this Home does know how to read.
    let odd = json!([{ "price": 0.2, "quantity": 1, "payToken": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913" }]);
    let items = market_catalog_from_payload(&row(odd, json!(0.2)), &usdc).unwrap();
    assert_eq!(items[0].price, "200000");

    // And when neither can be read, the card shows no price rather than a
    // number arrived at by guessing.
    let items = market_catalog_from_payload(&row(json!([]), json!("not a price")), &usdc).unwrap();
    assert_eq!(items[0].price, "0");
}

/// What the directory test server does with one accepted connection.
enum DirectoryReply {
    /// Accept and never answer: the request times out, as the first catalog
    /// read after a page load did on the installed Home (R49).
    Stall,
    /// Answer with this HTTP status and JSON body.
    Answer(u16, &'static str),
}

/// A local GraphQL endpoint answering each accepted connection in turn, and
/// how many connections it accepted.
async fn spawn_directory_server(
    replies: Vec<DirectoryReply>,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = accepted.clone();
    tokio::spawn(async move {
        let mut replies = replies.into_iter();
        let mut held = Vec::new();
        while let Ok((mut stream, _)) = listener.accept().await {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let Some(DirectoryReply::Answer(status, body)) = replies.next() else {
                held.push(stream);
                continue;
            };
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let read = stream.read(&mut buf).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    (format!("http://{addr}/graphql"), accepted)
}

fn short_directory_timeouts() -> OnchainDirectoryTimeouts {
    OnchainDirectoryTimeouts {
        connect: std::time::Duration::from_millis(250),
        total: std::time::Duration::from_millis(400),
    }
}

const DIRECTORY_CATALOG_ANSWER: &str = r#"{"data":{"result":{"total":0,"data":[]}}}"#;

fn approved_market_directory() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    MARKET_DIRECTORY
        .store_policy(dir.path(), "principal-directory-test", 1)
        .unwrap();
    dir
}

/// R49: a read whose first request dies in transport -- here it times out,
/// as the first catalog read after a page load did -- is asked once more with
/// a fresh client, and the second answer is the catalog.
#[tokio::test]
async fn test_directory_retries_once_after_a_transport_error() {
    let (endpoint, accepted) = spawn_directory_server(vec![
        DirectoryReply::Stall,
        DirectoryReply::Answer(200, DIRECTORY_CATALOG_ANSWER),
    ])
    .await;
    let dir = approved_market_directory();
    let items = fetch_market_catalog_at(dir.path(), &[], &endpoint, short_directory_timeouts())
        .await
        .expect("the retry answers");
    assert!(items.is_empty());
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// R49: two transport failures in a row are the catalog's existing
/// unavailable answer -- no items, a note, and no approval asked for -- and
/// never a third request. A connection refused is a transport error too.
#[tokio::test]
async fn test_directory_is_unavailable_after_two_transport_errors() {
    let (endpoint, accepted) =
        spawn_directory_server(vec![DirectoryReply::Stall, DirectoryReply::Stall]).await;
    let dir = approved_market_directory();
    let error = fetch_market_catalog_at(dir.path(), &[], &endpoint, short_directory_timeouts())
        .await
        .expect_err("two transport errors");
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
    let note = error.to_string();
    let answer = market_catalog_unavailable_answer(
        7,
        MARKET_DIRECTORY.note_should_request_approval(&note),
        &note,
    );
    assert_eq!(
        answer,
        json!({
            "asOf": 7,
            "unavailable": true,
            "needsApproval": false,
            "items": [],
            "note": note,
        })
    );

    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let refused = format!("http://{}/graphql", closed.local_addr().unwrap());
    drop(closed);
    assert!(
        fetch_market_catalog_at(dir.path(), &[], &refused, short_directory_timeouts())
            .await
            .is_err()
    );
}

/// R49: an HTTP answer is an answer. A 4xx or 5xx is reported as it came,
/// never asked again.
#[tokio::test]
async fn test_directory_does_not_retry_an_http_error_status() {
    for status in [422u16, 503] {
        let (endpoint, accepted) = spawn_directory_server(vec![
            DirectoryReply::Answer(status, r#"{"errors":[{"message":"no"}]}"#),
            DirectoryReply::Answer(200, DIRECTORY_CATALOG_ANSWER),
        ])
        .await;
        let dir = approved_market_directory();
        let error = fetch_market_catalog_at(dir.path(), &[], &endpoint, short_directory_timeouts())
            .await
            .expect_err("an error status is not retried into a success");
        assert!(error.to_string().contains(&status.to_string()), "{error}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

/// R49: each request is bounded by a connect timeout of its own, apart from
/// the whole request's, and at most two requests are made.
#[test]
fn test_directory_timeouts_separate_connect_from_total() {
    assert_eq!(
        OnchainDirectoryTimeouts::DEFAULT.connect,
        std::time::Duration::from_secs(4)
    );
    assert_eq!(
        OnchainDirectoryTimeouts::DEFAULT.total,
        std::time::Duration::from_secs(20)
    );
}
