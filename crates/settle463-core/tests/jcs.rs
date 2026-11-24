use serde_json::{json, Value};
use settle463_core::{
    canonical::{canonical, parse},
    ident::hex_string,
};

#[test]
fn unicode_order_and_literal_separators_match_published_bytes() {
    let g: Value = serde_json::from_str(include_str!(
        "../../../fixtures/golden/positive-signing-vector.json"
    ))
    .unwrap();
    assert_eq!(
        hex_string(&canonical(&json!({"\u{e000}":"pua","😀":"grin"})).unwrap()),
        g["unicode_key_order"]["canonical_utf8_hex"]
    );
    let line: Value = serde_json::from_str(include_str!(
        "../../../plan/golden/jcs-line-separators.json"
    ))
    .unwrap();
    for input in [
        line["input_json"].as_str().unwrap(),
        "{\"\u{2028}\":\"\u{2029}\"}",
    ] {
        assert_eq!(
            hex_string(&canonical(&parse(input.as_bytes()).unwrap()).unwrap()),
            line["canonical_utf8_hex"]
        );
    }
    assert_eq!(
        canonical(&json!({"k":"\0\u{1f}\t\n\r\u{8}\u{c}/\\\""})).unwrap(),
        br#"{"k":"\u0000\u001f\t\n\r\b\f/\\\""}"#
    );
}
