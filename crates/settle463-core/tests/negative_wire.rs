use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use settle463_core::{
    ident::{decimal, hex, hex_string, timestamp},
    parse_signed, signing_preimage,
};
use std::{fs, path::Path};

const OFFER: &[u8] = include_bytes!("../../../spec/examples/worked-transaction/service_offer.json");

#[test]
fn rejects_negative_wire_fixtures() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/negative");
    for name in [
        "uppercase-hex",
        "duplicate-key",
        "unknown-field",
        "wrong-version",
        "wrong-environment",
        "number-token",
        "seller-as-verifier",
        "equal-deadlines",
        "wrong-signature",
    ] {
        let raw = fs::read(dir.join(format!("{name}.json"))).unwrap();
        assert!(parse_signed(&raw, "sim:fixture").is_err(), "{name}");
    }
    let signed = parse_signed(OFFER, "sim:fixture").unwrap();
    let mut m: Value = serde_json::from_slice(OFFER).unwrap();
    m["signature"] = hex_string(
        &SigningKey::from_bytes(&[7; 32])
            .sign(&signing_preimage(signed.unsigned()).unwrap())
            .to_bytes(),
    )
    .into();
    assert!(parse_signed(&serde_json::to_vec(&m).unwrap(), "sim:fixture").is_err());
}

#[test]
fn rejects_objects_disguised_as_positional_arrays() {
    let m: Value = serde_json::from_slice(OFFER).unwrap();
    let positional = json!([
        m["protocol"],
        m["version"],
        m["environment"],
        m["issuer"],
        m["kind"],
        m["body"],
        m["signature"]
    ]);
    assert!(parse_signed(&serde_json::to_vec(&positional).unwrap(), "sim:fixture").is_err());
    for replacement in [
        json!(["application/json", "1048576", "sha256-json-object-v1"]),
        Value::Null,
        json!({"content_type":"application/json","max_bytes":"1048576","schema_profile":"sha256-json-object-v1","extra":false}),
    ] {
        let mut edited = m.clone();
        edited["body"]["input_profile"] = replacement;
        assert!(parse_signed(&serde_json::to_vec(&edited).unwrap(), "sim:fixture").is_err());
    }
}

#[test]
fn numeric_and_hex_boundaries() {
    assert_eq!(decimal("18446744073709551615").unwrap(), u64::MAX);
    assert_eq!(timestamp("9223372036854775807").unwrap(), i64::MAX);
    assert!(timestamp("9223372036854775808").is_err());
    for s in [
        "",
        "-0",
        " 1",
        "+1",
        "00",
        "1.0",
        "1e0",
        "18446744073709551616",
    ] {
        assert!(decimal(s).is_err(), "{s}");
    }
    assert!(hex::<1>("AA").is_err());
}

#[test]
fn byte_comparison_requires_observations_even_for_failures() {
    use settle463_core::{sign, wire::Body};
    let verdict = parse_signed(
        include_bytes!("../../../spec/examples/worked-transaction/verdict.json"),
        "sim:fixture",
    )
    .unwrap();
    let mut unsigned = verdict.unsigned().clone();
    let g: Value = serde_json::from_str(include_str!(
        "../../../fixtures/golden/positive-signing-vector.json"
    ))
    .unwrap();
    let key =
        SigningKey::from_bytes(&hex(g["keys"]["verifier"]["seed_hex"].as_str().unwrap()).unwrap());
    if let Body::Verdict(v) = &mut unsigned.payload {
        v.decision = "FAIL".into();
        v.reason_codes = vec!["DIGEST_MISMATCH".into()];
        v.observed_digest = None;
        v.observed_length = None;
    }
    assert!(sign(unsigned.clone(), &key).is_err());
    if let Body::Verdict(v) = &mut unsigned.payload {
        v.decision = "INDETERMINATE".into();
        v.reason_codes = vec!["UNAVAILABLE".into()];
    }
    sign(unsigned.clone(), &key).unwrap();
    if let Body::Verdict(v) = &mut unsigned.payload {
        v.reason_codes = vec!["DEADLINE_EXPIRED".into(), "UNAVAILABLE".into()];
    }
    assert!(sign(unsigned.clone(), &key).is_err());
    if let Body::Verdict(v) = &mut unsigned.payload {
        v.decision = "FAIL".into();
        v.reason_codes = vec!["OVERSIZED".into()];
    }
    sign(unsigned, &key).unwrap();
}
