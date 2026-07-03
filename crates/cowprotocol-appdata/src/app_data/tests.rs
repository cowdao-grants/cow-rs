use {
    super::*,
    ::cid::multihash::Multihash,
    alloy_primitives::{address, b256},
};

#[test]
fn json_round_trip_zero() {
    let zero = AppDataHash::default();
    let json = serde_json::to_value(zero).unwrap();
    assert_eq!(
        json,
        serde_json::json!("0x0000000000000000000000000000000000000000000000000000000000000000")
    );
    let parsed: AppDataHash = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, zero);
}

#[test]
fn json_round_trip_non_zero() {
    let mut bytes = [0u8; 32];
    bytes[0] = 0xab;
    bytes[31] = 0xcd;
    let original = AppDataHash::from(bytes);
    let json = serde_json::to_value(original).unwrap();
    let parsed: AppDataHash = serde_json::from_value(json).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn rejects_wrong_length() {
    let json = serde_json::json!("0xabcd");
    let result: Result<AppDataHash, _> = serde_json::from_value(json);
    assert!(result.is_err());
}

/// Lock [`EMPTY_APP_DATA_HASH`] against `keccak256("{}")`: any drift
/// would either break interop with cow-sdk fixtures or signal that the
/// canonical empty document changed.
#[test]
fn empty_app_data_hash_matches_keccak() {
    let computed = alloy_primitives::keccak256(EMPTY_APP_DATA_JSON);
    assert_eq!(EMPTY_APP_DATA_HASH, computed);
}

/// SDK-attribution doc pins appCode + metadata.quote.version. The
/// orderbook indexer reads these fields to count which integrators
/// produced an order; a regression here means orders silently stop
/// being attributable. Build the canonical JSON, then parse it
/// back so we lock both the in-memory builder and the wire shape.
#[test]
fn sdk_attribution_doc_pins_app_code_and_version() {
    for app_code in [COW_RS_APP_CODE, COW_RS_WASM_APP_CODE] {
        let doc = AppDataDoc::sdk_attribution(app_code);
        assert_eq!(doc.app_code.as_deref(), Some(app_code));

        let json = doc.canonical_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["appCode"], app_code);
        assert_eq!(
            value["metadata"]["quote"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(value["version"], LATEST_APP_DATA_VERSION);

        // Hash is stable across calls for a given app_code (and
        // changes when the app_code does).
        assert_eq!(
            AppDataDoc::sdk_attribution(app_code).hash(),
            AppDataDoc::sdk_attribution(app_code).hash(),
        );
    }
    assert_ne!(
        AppDataDoc::sdk_attribution(COW_RS_APP_CODE).hash(),
        AppDataDoc::sdk_attribution(COW_RS_WASM_APP_CODE).hash(),
        "cow-rs and cow-rs-wasm must produce distinct app-data digests"
    );
}

#[test]
fn empty_doc_matches_constant() {
    let doc = AppDataDoc::new("");
    // Every metadata field is `None` (or empty) by default.
    assert!(doc.metadata.quote.is_none());
    assert!(doc.metadata.order_class.is_none());
    assert!(doc.metadata.partner_fee.is_none());
    assert!(doc.metadata.referrer.is_none());
    assert!(doc.metadata.utm.is_none());
    assert!(doc.metadata.hooks.is_none());
    assert!(doc.metadata.flashloan.is_none());
    assert!(doc.metadata.replaced_order.is_none());
    assert!(doc.metadata.wrappers.is_empty());

    let json = doc.canonical_json();
    assert_eq!(json, r#"{"appCode":"","metadata":{},"version":"1.6.0"}"#);

    // Round-trip back into a doc.
    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, doc);
}

#[test]
fn referrer_doc_round_trips() {
    let referrer = address!("1234567890AbcdEF1234567890aBcdef12345678");
    let doc = AppDataDoc::new("my-app").with_referrer(referrer);

    let json = doc.canonical_json();
    // alloy's `Address` serialises with EIP-55 mixed-case checksum, so
    // assert on the case-insensitive hex rather than a literal string.
    assert!(
        json.to_lowercase()
            .contains(r#""referrer":{"address":"0x1234567890abcdef1234567890abcdef12345678"}"#)
    );

    let hash = doc.hash();
    // Re-hash from the JSON string and compare: guards against any
    // path where canonical_json and hash drift apart.
    let direct = alloy_primitives::keccak256(json.as_bytes());
    assert_eq!(hash, direct);

    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    let parsed_referrer = parsed.metadata.referrer.expect("referrer preserved");
    assert_eq!(parsed_referrer.address, referrer);
}

/// Golden vector: locks the canonical bytes and hash of the minimal
/// `AppDataDoc::new("")` document. Computed independently with Python
/// (`json.dumps(..., sort_keys=True, separators=(",", ":"))` + keccak)
///: any drift here means our deterministic serialisation changed and
/// will silently re-hash existing fixtures.
#[test]
fn minimal_doc_golden_hash() {
    let doc = AppDataDoc::new("");
    assert_eq!(
        doc.canonical_json(),
        r#"{"appCode":"","metadata":{},"version":"1.6.0"}"#
    );
    let expected = b256!("3929e2c230dc41c0c053ff5f9211eb32def3a737b2bf36eb5b8862ea317fcd9e");
    assert_eq!(doc.hash(), expected);
}

/// `canonical_json` must emit non-ASCII characters as raw UTF-8
/// bytes (matching the orderbook's
/// `keccak256(toUtf8Bytes(fullAppData))` digest input), not as
/// `\uXXXX` ASCII escapes. A regression that flips the escape mode
/// would silently re-hash every document containing non-ASCII
/// content (utm campaigns with emoji, non-Latin appCodes) and orders
/// signed against the old digest would be rejected by the orderbook.
#[test]
fn canonical_json_preserves_utf8_non_ascii_bytes() {
    // Build a doc whose appCode contains a non-ASCII character.
    let doc = AppDataDoc::new("café-\u{1F40c}"); // "café-🐌"
    let json = doc.canonical_json();

    // Raw UTF-8: the `é` byte sequence (0xc3 0xa9) and the snail
    // emoji (4 bytes) must appear verbatim in the canonical JSON,
    // and NOT as `é` / `🐌` escapes.
    assert!(
        json.contains("café-\u{1F40c}"),
        "expected raw UTF-8 non-ASCII bytes in canonical JSON, got: {json}"
    );
    assert!(
        !json.contains("\\u00e9"),
        "expected raw UTF-8, found ASCII escape: {json}"
    );
    assert!(
        !json.contains("\\ud83d"),
        "expected raw UTF-8, found surrogate-pair escape: {json}"
    );

    // The digest is `keccak256(canonical_json.as_bytes())`. Lock
    // it against the bytes produced by the current path so any
    // serialiser flip (raw → escaped) trips this test.
    let direct = alloy_primitives::keccak256(json.as_bytes());
    assert_eq!(doc.hash(), direct);

    // Round-trip: parsing back through serde must reconstruct the
    // same appCode bytes.
    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.app_code.as_deref(), Some("café-\u{1F40c}"));
}

#[test]
fn canonical_json_sorts_keys_deterministically() {
    // Build a doc with several metadata fields and confirm the
    // emitted JSON is sorted at every level.
    let doc = AppDataDoc::new("app")
        .with_referrer(address!("0000000000000000000000000000000000000001"))
        .with_partner_fee(50, address!("0000000000000000000000000000000000000002"))
        .with_order_class(OrderClass::Limit)
        .with_slippage_bips(25)
        .with_environment("prod");

    let json = doc.canonical_json();

    // Top-level keys appear in lexicographic order.
    let app_code_pos = json.find("appCode").unwrap();
    let environment_pos = json.find("environment").unwrap();
    let metadata_pos = json.find("metadata").unwrap();
    let version_pos = json.find("\"version\"").unwrap();
    assert!(app_code_pos < environment_pos);
    assert!(environment_pos < metadata_pos);
    assert!(metadata_pos < version_pos);

    // Nested keys are sorted too. `AppDataMetadata` declares these in
    // the order quote, orderClass, partnerFee, referrer; emitting them
    // lexicographically (orderClass, partnerFee, quote, referrer) proves
    // the serialiser is not preserving declaration/insertion order. This
    // is the assertion that trips if serde_json's `preserve_order`
    // feature is ever switched on.
    let order_class_pos = json.find("orderClass").unwrap();
    let partner_fee_pos = json.find("partnerFee").unwrap();
    let quote_pos = json.find("\"quote\"").unwrap();
    let referrer_pos = json.find("referrer").unwrap();
    assert!(order_class_pos < partner_fee_pos);
    assert!(partner_fee_pos < quote_pos);
    assert!(quote_pos < referrer_pos);

    // The output is stable run-to-run.
    assert_eq!(json, doc.canonical_json());
}

#[test]
fn partner_fee_volume_round_trips_with_legacy_bps_key() {
    let recipient = address!("00000000219AB540356CBb839CbE05303D7705FA");
    let doc = AppDataDoc::new("app").with_partner_fee(75, recipient);
    let json = doc.canonical_json();
    // Volume serialises with the legacy `"bps"` key so existing
    // app-data hashes stay stable. The `recipient` is in the same
    // flat object, not nested under `policy`.
    assert!(
        json.to_lowercase()
            .contains(r#""partnerfee":{"bps":75,"recipient":"#),
        "got: {json}",
    );
    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    let fee = parsed.metadata.partner_fee.expect("partner fee preserved");
    assert!(matches!(fee.policy(), FeePolicy::Volume { bps: 75 }));
    assert_eq!(fee.recipient(), recipient);
}

/// Surplus policy emits `surplusBps` + `maxVolumeBps` flat alongside
/// `recipient`. Locks the wire shape against the upstream
/// `FeePolicy` deserializer.
#[test]
fn partner_fee_surplus_emits_typed_keys() {
    let recipient = address!("00000000219AB540356CBb839CbE05303D7705FA");
    let doc = AppDataDoc::new("app").with_partner_fee_policy(
        FeePolicy::Surplus {
            bps: 25,
            max_volume_bps: 100,
        },
        recipient,
    );
    let json = doc.canonical_json();
    assert!(json.contains(r#""maxVolumeBps":100"#), "got: {json}");
    assert!(json.contains(r#""surplusBps":25"#), "got: {json}");

    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    let fee = parsed.metadata.partner_fee.expect("partner fee preserved");
    assert!(matches!(
        fee.policy(),
        FeePolicy::Surplus {
            bps: 25,
            max_volume_bps: 100,
        }
    ));
}

/// PriceImprovement policy emits `priceImprovementBps` +
/// `maxVolumeBps`.
#[test]
fn partner_fee_price_improvement_emits_typed_keys() {
    let recipient = address!("00000000219AB540356CBb839CbE05303D7705FA");
    let doc = AppDataDoc::new("app").with_partner_fee_policy(
        FeePolicy::PriceImprovement {
            bps: 30,
            max_volume_bps: 150,
        },
        recipient,
    );
    let json = doc.canonical_json();
    assert!(json.contains(r#""priceImprovementBps":30"#), "got: {json}");
    assert!(json.contains(r#""maxVolumeBps":150"#), "got: {json}");

    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    let fee = parsed.metadata.partner_fee.expect("partner fee preserved");
    assert!(matches!(
        fee.policy(),
        FeePolicy::PriceImprovement {
            bps: 30,
            max_volume_bps: 150,
        }
    ));
}

/// Volume can also be expressed as `volumeBps` on the wire, which
/// the upstream deserializer accepts. We accept it too.
#[test]
fn partner_fee_deserialises_volume_bps_alias() {
    let recipient = address!("00000000219AB540356CBb839CbE05303D7705FA");
    let json = format!(r#"{{"volumeBps":42,"recipient":"{recipient:?}"}}"#,);
    let fee: AppDataPartnerFee = serde_json::from_str(&json).unwrap();
    assert!(matches!(fee.policy(), FeePolicy::Volume { bps: 42 }));
    assert_eq!(fee.recipient(), recipient);
}

/// An over-cap `bps` cannot reach a signed digest. The builder defers
/// the check, so finalisation (`try_hash` / `hash` / `prepare`) is what
/// refuses to fold an attacker-controlled partner-fee tier into the
/// app-data hash.
#[test]
fn partner_fee_over_cap_rejected_at_finalisation() {
    let recipient = address!("00000000219AB540356CBb839CbE05303D7705FA");

    let err = AppDataDoc::new("app")
        .with_partner_fee(10_001, recipient)
        .try_hash()
        .unwrap_err();
    assert!(matches!(
        err,
        AppDataError::FeeOutOfRange {
            field: "bps",
            value: 10_001,
            max: PARTNER_FEE_BPS_MAX,
        }
    ));

    // `with_partner_fee_policy` walks every typed variant.
    let err = AppDataDoc::new("app")
        .with_partner_fee_policy(
            FeePolicy::Surplus {
                bps: 1,
                max_volume_bps: 10_001,
            },
            recipient,
        )
        .try_hash()
        .unwrap_err();
    assert!(matches!(
        err,
        AppDataError::FeeOutOfRange {
            field: "maxVolumeBps",
            value: 10_001,
            ..
        }
    ));

    let err = AppDataDoc::new("app")
        .with_partner_fee_policy(
            FeePolicy::PriceImprovement {
                bps: 10_001,
                max_volume_bps: 1,
            },
            recipient,
        )
        .prepare()
        .unwrap_err();
    assert!(matches!(
        err,
        AppDataError::FeeOutOfRange {
            field: "priceImprovementBps",
            value: 10_001,
            ..
        }
    ));

    // At the cap is still accepted.
    let _ = AppDataDoc::new("app")
        .with_partner_fee(PARTNER_FEE_BPS_MAX, recipient)
        .try_hash()
        .expect("bps at the cap must be accepted");
}

/// Ambiguous policy combinations are rejected outright.
#[test]
fn partner_fee_rejects_mixed_policy_fields() {
    let recipient = address!("00000000219AB540356CBb839CbE05303D7705FA");
    let json = format!(
        r#"{{"surplusBps":10,"priceImprovementBps":20,"maxVolumeBps":50,"recipient":"{recipient:?}"}}"#,
    );
    let err = serde_json::from_str::<AppDataPartnerFee>(&json).unwrap_err();
    assert!(err.to_string().contains("unknown partner-fee policy"));
}

/// Lock the wire shape and round-trip of `metadata.flashloan`.
#[test]
fn flashloan_round_trips() {
    let flashloan = AppDataFlashloan {
        liquidity_provider: address!("1111111111111111111111111111111111111111"),
        protocol_adapter: address!("2222222222222222222222222222222222222222"),
        receiver: address!("3333333333333333333333333333333333333333"),
        token: address!("4444444444444444444444444444444444444444"),
        amount: U256::from(1_000_000_u64),
    };
    let doc = AppDataDoc::new("app").with_flashloan(flashloan.clone());
    let json = doc.canonical_json();
    assert!(
        json.contains(r#""amount":"1000000""#),
        "amount must serialise as decimal string, got: {json}",
    );
    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(
        parsed.metadata.flashloan.expect("flashloan preserved"),
        flashloan
    );
}

/// `metadata.replacedOrder.uid` round-trips through the wire form.
#[test]
fn replaced_order_round_trips() {
    let uid = OrderUid::from([0x55; 56]);
    let doc = AppDataDoc::new("app").with_replaced_order(uid);
    let json = doc.canonical_json();
    assert!(
        json.contains(r#""replacedOrder":{"uid":"0x"#),
        "got: {json}"
    );
    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(
        parsed.metadata.replaced_order.expect("replaced order").uid,
        uid
    );
}

/// `metadata.wrappers[]` round-trips with hex-encoded call data, and
/// is skipped from the canonical JSON when empty (preserving the
/// digest of documents authored before this field existed).
#[test]
fn wrappers_round_trip_and_skip_when_empty() {
    // Empty wrappers must not appear in canonical JSON, otherwise
    // the document's hash drifts away from what older SDKs computed.
    let doc = AppDataDoc::new("app");
    assert!(!doc.canonical_json().contains("wrappers"));

    let wrapper = AppDataWrapperCall {
        address: address!("5555555555555555555555555555555555555555"),
        data: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
        is_omittable: true,
    };
    let doc = doc.with_wrapper(wrapper.clone());
    let json = doc.canonical_json();
    assert!(json.contains(r#""data":"0xdeadbeef""#), "got: {json}");
    assert!(json.contains(r#""isOmittable":true"#), "got: {json}");
    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.metadata.wrappers, vec![wrapper]);
}

#[test]
fn order_class_serialises_as_lowercase() {
    let doc = AppDataDoc::new("app").with_order_class(OrderClass::Market);
    let json = doc.canonical_json();
    assert!(json.contains(r#""orderClass":{"orderClass":"market"}"#));
}

#[test]
fn hooks_typed_round_trip() {
    use alloy_primitives::Bytes;

    let target = address!("0000000000000000000000000000000000000abc");
    let hooks = AppDataHooks {
        version: Some("0.1.0".to_string()),
        pre: vec![Hook {
            target,
            call_data: Bytes::from_static(&[0xde, 0xf0]),
            gas_limit: "21000".to_string(),
        }],
        post: Vec::new(),
    };
    let doc = AppDataDoc::new("app").with_hooks(hooks.clone());
    let json = doc.canonical_json();
    // Camel-cased keys; `pre`/`post` are always emitted, sorted before
    // `version`.
    assert!(
        json.contains(r#""hooks":{"post":[],"pre":[{"callData":"0xdef0","gasLimit":"21000","#),
        "got: {json}",
    );

    let parsed: AppDataDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.metadata.hooks, Some(hooks));
}

#[test]
fn prepare_bundles_match_granular_accessors() {
    let doc = AppDataDoc::new("app")
        .with_referrer(address!("0000000000000000000000000000000000000001"))
        .with_environment("prod");
    let prepared = doc.prepare().expect("prepare");
    assert_eq!(prepared.full_app_data, doc.canonical_json());
    assert_eq!(prepared.hash, doc.hash());
    assert_eq!(prepared.cid, app_data_cid(doc.hash()));
}

#[test]
fn prepare_rejects_over_cap_partner_fee() {
    let recipient = address!("00000000219AB540356CBb839CbE05303D7705FA");
    let err = AppDataDoc::new("app")
        .with_partner_fee(10_001, recipient)
        .prepare()
        .unwrap_err();
    assert!(matches!(err, AppDataError::FeeOutOfRange { .. }));
}

/// Round-trip every byte position so any off-by-one in the base32 packer
/// or in the `app_data_hash_from_cid` recovery shows up immediately.
#[test]
fn cid_round_trip_default_and_walking_bytes() {
    let default = AppDataHash::default();
    assert_eq!(
        app_data_hash_from_cid(&app_data_cid(default)).unwrap(),
        default
    );

    for i in 0..32 {
        let mut bytes = [0u8; 32];
        bytes[i] = 0xff;
        let hash = AppDataHash::from(bytes);
        let cid = app_data_cid(hash);
        let rendered = cid.to_string();
        assert_eq!(
            app_data_hash_from_cid(&cid).unwrap(),
            hash,
            "round-trip failed at byte {i}"
        );
        // Multibase tag is always `b`, body is always lower-case alpha
        // or `234567`, never any other character.
        assert!(rendered.starts_with('b'));
        assert!(
            rendered
                .chars()
                .skip(1)
                .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c))
        );
    }
}

/// `bafkrw` is the base32 multibase rendering of the constant CID prefix
/// `01 55 1b 20` (CIDv1, raw codec, keccak-256, 32 bytes). Any digest
/// that uses this prefix must encode to a string starting with
/// `bafkrw`. The legacy sha2-256 path that gives `bafkrei` is **not**
/// what the orderbook pins under.
#[test]
fn cid_for_empty_app_data_hash_starts_with_bafkrw() {
    let cid = app_data_cid(EMPTY_APP_DATA_HASH).to_string();
    assert!(
        cid.starts_with("bafkrw"),
        "expected bafkrw prefix, got {cid}"
    );
}

/// Golden vector lifted from `cowprotocol/services`
/// (`crates/app-data/src/app_data_hash.rs::tests::known_good`). The
/// services repo additionally cites the equivalent `ipfs cid format
/// -b base16` and `-b base32` strings, both of which we lock here.
#[test]
fn cid_matches_services_known_good_vector() {
    let hash = b256!("8af4e8c9973577b08ac21d17d331aade86c11ebcc5124744d621ca8365ec9424");
    let cid = app_data_cid(hash);
    assert_eq!(
        cid.to_string(),
        "bafkrwiek6tumtfzvo6yivqq5c7jtdkw6q3ar5pgfcjdujvrbzkbwl3eueq"
    );
    assert_eq!(app_data_hash_from_cid(&cid).unwrap(), hash);
}

/// Lock the canonical CID for the default-empty document
/// (`keccak256("{}")`), independently computed via Python's `base32`
/// over `01 55 1b 20 || EMPTY_APP_DATA_HASH`.
#[test]
fn cid_for_empty_doc_golden() {
    let cid = app_data_cid(EMPTY_APP_DATA_HASH);
    assert_eq!(
        cid.to_string(),
        "bafkrwifuru4pspvkbbadh7czoc7znzkzym6ezxah3ce2wafu2y7zledttu"
    );
}

#[test]
fn cid_parse_rejects_missing_multibase_prefix() {
    // Drop the leading `b`. `cid::Cid::from_str` rejects via the
    // upstream multibase decoder (no prefix => parse failure).
    let err = "afkrwifuru4pspvkbbadh7czoc7znzkzym6ezxah3ce2wafu2y7zledttu"
        .parse::<AppDataCid>()
        .unwrap_err();
    assert!(matches!(err, ::cid::Error::ParsingError), "got: {err:?}");
}

/// Round-trip the same `services` golden vector through the `f`
/// (base16) multibase prefix. cow-sdk's TypeScript
/// `appDataHexToCid` emits this form by default; the orderbook
/// accepts either prefix, so cow-rs must too.
#[test]
fn cid_parse_accepts_base16_multibase_prefix() {
    let hash = b256!("8af4e8c9973577b08ac21d17d331aade86c11ebcc5124744d621ca8365ec9424");
    let mut hex_body = String::with_capacity(72);
    hex_body.push_str("01551b20");
    hex_body.push_str(&const_hex::encode(hash));
    let cid = format!("f{hex_body}").parse::<AppDataCid>().unwrap();
    assert_eq!(app_data_hash_from_cid(&cid).unwrap(), hash);
}

#[test]
fn cid_parse_rejects_invalid_base16_body() {
    let err = "f01551b20zzzz".parse::<AppDataCid>().unwrap_err();
    assert!(matches!(err, ::cid::Error::ParsingError), "got: {err:?}");
}

/// `parse_app_data_cid` rejects an oversized string before the
/// upstream multibase decoder allocates for it, capping the work a
/// hostile CID can force.
#[test]
fn parse_app_data_cid_rejects_oversize_string() {
    let oversize = format!("b{}", "a".repeat(MAX_CID_STR_LEN));
    let err = parse_app_data_cid(&oversize).unwrap_err();
    assert!(
        matches!(err, AppDataCidError::CidTooLong { max, .. } if max == MAX_CID_STR_LEN),
        "got: {err:?}"
    );
}

/// A well-formed CID under the cap still parses through the bounded
/// helper, so the guard does not reject legitimate input.
#[test]
fn parse_app_data_cid_accepts_canonical_string() {
    let cid = app_data_cid(EMPTY_APP_DATA_HASH).to_string();
    assert!(cid.len() <= MAX_CID_STR_LEN);
    let parsed = parse_app_data_cid(&cid).unwrap();
    assert_eq!(
        app_data_hash_from_cid(&parsed).unwrap(),
        EMPTY_APP_DATA_HASH
    );
}

#[test]
fn cid_parse_rejects_wrong_codec() {
    // dag-pb (0x70) codec instead of raw (0x55). Build via the cid
    // crate so the byte layout matches what a real CID parser sees.
    let multihash =
        Multihash::<32>::wrap(MULTIHASH_KECCAK_256, EMPTY_APP_DATA_HASH.as_slice()).unwrap();
    let cid = AppDataCid::new_v1(0x70, multihash.resize().unwrap());
    let err = app_data_hash_from_cid(&cid).unwrap_err();
    assert!(
        matches!(err, AppDataCidError::UnexpectedCodec(0x70)),
        "got: {err:?}"
    );
}

#[test]
fn cid_parse_rejects_wrong_multihash() {
    // sha2-256 (0x12) instead of keccak-256 (0x1b): this is the
    // distinct "legacy" CID family that cow-sdk's `appDataHexToCidLegacy`
    // emits. We do not want to silently accept it as our CID since
    // its digest semantics are different.
    let multihash = Multihash::<32>::wrap(0x12, EMPTY_APP_DATA_HASH.as_slice()).unwrap();
    let cid = AppDataCid::new_v1(CID_CODEC_RAW, multihash.resize().unwrap());
    let err = app_data_hash_from_cid(&cid).unwrap_err();
    assert!(
        matches!(err, AppDataCidError::UnexpectedMultihashCode(0x12)),
        "got: {err:?}"
    );
}

#[test]
fn cid_parse_rejects_truncated_body() {
    // Body too short to even contain a valid CID header. The cid
    // parser raises one of several syntactic errors depending on
    // exactly where the truncation lands; all are acceptable here.
    let err = "babcdefgh".parse::<AppDataCid>().unwrap_err();
    assert!(
        matches!(
            err,
            ::cid::Error::ParsingError
                | ::cid::Error::VarIntDecodeError
                | ::cid::Error::InputTooShort
                | ::cid::Error::InvalidExplicitCidV0
                | ::cid::Error::Io(_)
        ),
        "got: {err:?}"
    );
}

#[test]
fn cid_parse_rejects_wrong_version() {
    // Version byte `0x00` is the CIDv0 marker. The cid crate rejects
    // it in CIDv1-shaped input via `InvalidExplicitCidV0`.
    let bytes = [
        0x00, 0x55, 0x1b, 0x20, 0xb4, 0x8d, 0x38, 0xf9, 0x3e, 0xaa, 0x08, 0x40, 0x33, 0xfc, 0x59,
        0x70, 0xbf, 0x96, 0xe5, 0x59, 0xc3, 0x3c, 0x4c, 0xdc, 0x07, 0xd8, 0x89, 0xab, 0x00, 0xb4,
        0xd6, 0x3f, 0x95, 0x90, 0x73, 0x9d,
    ];
    let encoded = ::cid::multibase::encode(::cid::multibase::Base::Base32Lower, bytes);
    let err = encoded.parse::<AppDataCid>().unwrap_err();
    assert!(
        matches!(err, ::cid::Error::InvalidExplicitCidV0),
        "got: {err:?}"
    );
}

#[test]
fn cid_parse_rejects_wrong_digest_length() {
    // Multihash length 16 (0x10) instead of 32 (0x20).
    let multihash =
        Multihash::<32>::wrap(MULTIHASH_KECCAK_256, &EMPTY_APP_DATA_HASH.as_slice()[..16]).unwrap();
    let cid = AppDataCid::new_v1(CID_CODEC_RAW, multihash.resize().unwrap());
    let err = app_data_hash_from_cid(&cid).unwrap_err();
    assert!(
        matches!(err, AppDataCidError::UnexpectedDigestLength(16)),
        "got: {err:?}"
    );
}

#[test]
fn cid_parse_rejects_invalid_base32_char() {
    // `8` is outside RFC 4648's lower-case 32-char alphabet.
    let err = "b8".parse::<AppDataCid>().unwrap_err();
    assert!(
        matches!(
            err,
            ::cid::Error::ParsingError | ::cid::Error::InputTooShort
        ),
        "got: {err:?}"
    );
}
