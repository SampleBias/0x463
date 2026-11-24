use ed25519_dalek::{Signature, SigningKey};
use serde_json::Value;
use settle463_core::{
    canonical, check_quote_request,
    ident::{base58, hex, hex_string},
    message_id, order_id, parse_signed, policy_hash, sign, signing_preimage,
};
use std::{fs, path::Path};

const GOLDEN: &str = include_str!("../../../fixtures/golden/positive-signing-vector.json");

#[test]
fn independent_golden_vector_and_one_byte_mutation() {
    let g: Value = serde_json::from_str(GOLDEN).unwrap();
    let v = &g["signing"];
    let signed = parse_signed(
        &serde_json::to_vec(&v["signed_message"]).unwrap(),
        "sim:fixture",
    )
    .unwrap();
    assert_eq!(
        canonical::canonical(&v["unsigned_object"]).unwrap(),
        v["canonical_utf8"].as_str().unwrap().as_bytes()
    );
    let mut preimage = signing_preimage(signed.unsigned()).unwrap();
    assert_eq!(hex_string(&preimage), v["preimage_hex"]);
    assert_eq!(hex_string(signed.id()), v["message_id"]);
    let key =
        SigningKey::from_bytes(&hex(g["keys"]["seller"]["seed_hex"].as_str().unwrap()).unwrap());
    assert_eq!(
        base58::<32>(g["keys"]["seller"]["public_key_base58"].as_str().unwrap()).unwrap(),
        key.verifying_key().to_bytes()
    );
    let reproduced = sign(signed.unsigned().clone(), &key).unwrap();
    assert_eq!(hex_string(reproduced.signature()), v["signature_hex"]);
    assert_eq!(
        reproduced.to_wire().unwrap(),
        canonical::canonical(&v["signed_message"]).unwrap()
    );
    *preimage.last_mut().unwrap() ^= 1;
    assert_ne!(&message_id(&preimage), signed.id());
    assert!(key
        .verifying_key()
        .verify_strict(&preimage, &Signature::from_bytes(signed.signature()))
        .is_err());

    let order = &g["order"]["object"];
    assert_eq!(
        hex_string(
            &order_id(
                "sim:fixture",
                order["buyer"].as_str().unwrap(),
                order["nonce"].as_str().unwrap()
            )
            .unwrap()
        ),
        g["order"]["order_id"]
    );
    let policy = serde_json::from_value(g["policy"]["object"].clone()).unwrap();
    assert_eq!(
        hex_string(&policy_hash(&policy).unwrap()),
        g["policy"]["policy_hash"]
    );
}

#[test]
fn every_worked_id_and_the_quote_ceiling() {
    let folder =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/worked-transaction");
    let table = fs::read_to_string(folder.join("README.md")).unwrap();
    let load = |kind: &str| {
        let raw = fs::read(folder.join(format!("{kind}.json"))).unwrap();
        let msg = parse_signed(&raw, "sim:fixture").unwrap();
        assert!(table.contains(&format!(
            "| [{kind}]({kind}.json) | `{}` |",
            hex_string(msg.id())
        )));
        msg
    };
    for name in ["service_offer", "accept", "delivery", "verdict", "receipt"] {
        load(name);
    }
    let request = load("request");
    check_quote_request(&load("quote"), &request).unwrap();
    let over = parse_signed(
        include_bytes!("../../../spec/examples/negative/above-ceiling.json"),
        "sim:fixture",
    )
    .unwrap();
    assert!(check_quote_request(&over, &request).is_err());
}
