//! Inspect raw artifact fields without collapsing duplicate JSON property names.

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;
use std::{collections::HashSet, fmt};

pub const MAX_BYTES: usize = 1_048_576;

pub enum FieldValue {
    String(String),
    Bool,
    Nested,
    Other,
}

pub struct Object {
    pub fields: Vec<(String, FieldValue)>,
    pub duplicate: bool,
}

struct Members<'a>(Vec<(String, &'a RawValue)>);
impl<'de> Deserialize<'de> for Members<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = Members<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = Vec::new();
                while let Some(member) = map.next_entry()? {
                    fields.push(member);
                }
                Ok(Members(fields))
            }
        }
        d.deserialize_map(ObjectVisitor)
    }
}

// RawValue has checked the JSON grammar. Scan tokens once to find nested duplicate
// names, even when an object is inside an array. No recursion or numeric conversion is needed.
fn nested_duplicate(raw: &str) -> serde_json::Result<bool> {
    let bytes = raw.as_bytes();
    let mut scopes: Vec<HashSet<String>> = Vec::new();
    let mut duplicate = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => scopes.push(HashSet::new()),
            b'}' => {
                scopes.pop();
            }
            b'"' => {
                let start = i;
                i += 1;
                while bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                let string: String = serde_json::from_str(&raw[start..=i])?;
                let mut next = i + 1;
                while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                    next += 1;
                }
                if bytes.get(next) == Some(&b':') {
                    if let Some(keys) = scopes.last_mut() {
                        duplicate |= !keys.insert(string);
                    }
                }
            }
            _ => (),
        }
        i += 1;
    }
    Ok(duplicate)
}

pub fn object(bytes: &[u8]) -> crate::Result<Object> {
    if bytes.len() > MAX_BYTES {
        return Err(crate::Error::Invalid("artifact exceeds hard limit"));
    }
    let Members(entries) = serde_json::from_slice(bytes)?;
    let mut seen = HashSet::new();
    let mut duplicate = false;
    let mut fields = Vec::with_capacity(entries.len());
    for (key, value) in &entries {
        duplicate |= !seen.insert(key);
        let raw = value.get();
        let field = match raw.as_bytes()[0] {
            b'"' => FieldValue::String(serde_json::from_str(raw)?),
            b't' | b'f' => FieldValue::Bool,
            b'{' | b'[' => {
                duplicate |= nested_duplicate(raw)?;
                FieldValue::Nested
            }
            _ => FieldValue::Other,
        };
        fields.push((key.clone(), field));
    }
    Ok(Object { fields, duplicate })
}

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    crate::ident::hex_string(&Sha256::digest(bytes))
}
