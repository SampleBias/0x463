use settle463_core::{
    artifact::digest,
    wire::{Limits, Policy},
};
use settle463_verify::{declared_oversized, judge};
use std::collections::BTreeMap;

const GOOD: &[u8] = include_bytes!("../../../fixtures/artifacts/reference.json");
const EXTRA: &[u8] = include_bytes!("../../../fixtures/artifacts/extra-field.json");
fn policy(bytes: &[u8]) -> Policy {
    Policy {
        expected_digest: digest(bytes),
        expected_length: bytes.len().to_string(),
        profile: "sha256-json-object-v1".into(),
        schema: BTreeMap::from([
            ("job".into(), "string".into()),
            ("result".into(), "string".into()),
            ("units".into(), "u64_decimal".into()),
        ]),
        limits: Limits {
            max_bytes: "1048576".into(),
            max_fields: "64".into(),
            max_field_name_bytes: "64".into(),
            max_string_bytes: "4096".into(),
        },
    }
}
#[test]
fn reason_codes_and_raw_observations() {
    let p = policy(GOOD);
    let pass = judge(Some(GOOD), &p, 9, 10);
    assert_eq!(pass.decision, "PASS");
    assert_eq!(pass.reason_codes, ["MATCH"]);
    assert_eq!(pass.observed_length.as_deref(), Some("52"));
    assert_eq!(pass.observed_digest.as_ref(), Some(&p.expected_digest));
    for (bytes, expected) in [
        (
            include_bytes!("../../../fixtures/artifacts/one-byte-change.json").as_slice(),
            vec!["DIGEST_MISMATCH"],
        ),
        (
            include_bytes!("../../../fixtures/artifacts/number.json").as_slice(),
            vec!["DIGEST_MISMATCH", "FIELD_TYPE", "SIZE_MISMATCH"],
        ),
        (
            include_bytes!("../../../fixtures/artifacts/duplicate-key.json").as_slice(),
            vec!["DIGEST_MISMATCH", "DUPLICATE_FIELD", "SIZE_MISMATCH"],
        ),
        (
            include_bytes!("../../../fixtures/artifacts/malformed.json").as_slice(),
            vec!["DIGEST_MISMATCH", "MALFORMED_JSON", "SIZE_MISMATCH"],
        ),
        (
            EXTRA,
            vec!["DIGEST_MISMATCH", "SCHEMA_MISMATCH", "SIZE_MISMATCH"],
        ),
    ] {
        let fail = judge(Some(bytes), &p, 9, 10);
        assert_eq!(fail.decision, "FAIL");
        assert_eq!(fail.reason_codes, expected);
        assert_eq!(fail.observed_digest, Some(digest(bytes)));
    }
    assert_eq!(
        judge(Some(EXTRA), &policy(EXTRA), 9, 10).reason_codes,
        ["SCHEMA_MISMATCH"]
    );
}
#[test]
fn byte_limits_and_unavailable_or_expired_attempts() {
    let p = policy(GOOD);
    let mut tight = p.clone();
    tight.limits.max_bytes = GOOD.len().to_string();
    assert!(judge(Some(EXTRA), &tight, 9, 10)
        .reason_codes
        .contains(&"OVERSIZED".into()));
    let too_big = vec![b'x'; 1_048_577];
    assert_eq!(
        judge(Some(&too_big), &p, 9, 10).reason_codes,
        ["DIGEST_MISMATCH", "OVERSIZED", "SIZE_MISMATCH"]
    );
    assert_eq!(declared_oversized(9, 10).observed_length, None);
    assert_eq!(declared_oversized(9, 10).reason_codes, ["OVERSIZED"]);
    for bytes in [Some(GOOD), None] {
        let late = judge(bytes, &p, 10, 10);
        assert_eq!(late.decision, "INDETERMINATE");
        assert_eq!(late.reason_codes, ["DEADLINE_EXPIRED"]);
        assert!(late.observed_digest.is_none());
    }
    assert_eq!(judge(None, &p, 9, 10).reason_codes, ["UNAVAILABLE"]);
}
#[test]
fn types_duplicate_names_and_limits_use_decoded_fields() {
    let bytes =
        br#"{"job":"abc","result":[{"x":true,"\u0078":false}],"units":"03","extra-long":0}"#;
    let mut p = policy(bytes);
    p.limits.max_fields = "3".into();
    p.limits.max_field_name_bytes = "6".into();
    p.limits.max_string_bytes = "2".into();
    assert_eq!(
        judge(Some(bytes), &p, 0, 1).reason_codes,
        [
            "DUPLICATE_FIELD",
            "FIELD_LENGTH",
            "FIELD_TYPE",
            "SCHEMA_MISMATCH",
            "TOO_MANY_FIELDS"
        ]
    );
    let valid = br#"{"job":true,"result":"ok","units":"18446744073709551615"}"#;
    let mut p = policy(valid);
    p.schema.insert("job".into(), "bool".into());
    assert_eq!(judge(Some(valid), &p, 0, 1).decision, "PASS");
    let number = br#"{"job":1e999,"result":"ok","units":"2"}"#;
    assert_eq!(
        judge(Some(number), &policy(number), 0, 1).reason_codes,
        ["FIELD_TYPE"]
    );
}
