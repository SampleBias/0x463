use settle463_core::canonical::parse;

#[test]
fn rejects_invalid_protocol_json_before_deserializing_a_map() {
    for raw in [
        br#"{"a":"1","a":"2"}"#.as_slice(),
        br#"{"a":[{"x":true,"\u0078":false}]}"#,
        br#"{"a":1}"#,
        br#"[null]"#,
        br#""\ud800""#,
        b"\xff",
        b"{",
        b"false{}",
    ] {
        assert!(parse(raw).is_err(), "{raw:?}");
    }
    assert!(parse(b"{}").is_ok());
}
