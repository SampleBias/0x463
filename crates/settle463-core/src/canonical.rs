//! Strict protocol JSON and RFC 8785 serialization of its number-free subset.

use serde::de::{Deserialize, Deserializer, Error as _, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

use crate::{Error, Result};

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a protocol object, array, string or boolean")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<Value, E> {
        if value.len() > 4096 {
            return Err(E::custom("string exceeds 4096 UTF-8 bytes"));
        }
        Ok(Value::String(value.to_owned()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut input: A) -> std::result::Result<Value, A::Error> {
        let mut fields = Map::new();
        while let Some(key) = input.next_key::<String>()? {
            if key.len() > 4096 || fields.contains_key(&key) {
                return Err(A::Error::custom("duplicate or oversized property name"));
            }
            fields.insert(key, input.next_value::<Strict>()?.0);
        }
        Ok(Value::Object(fields))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> std::result::Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(Strict(item)) = input.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }
}

pub fn parse(bytes: &[u8]) -> Result<Value> {
    Ok(serde_json::from_slice::<Strict>(bytes)?.0)
}

pub fn canonical(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    write(value, &mut bytes)?;
    Ok(bytes)
}

fn write(value: &Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::String(s) if s.len() <= 4096 => serde_json::to_writer(out, s)?,
        Value::Bool(b) => serde_json::to_writer(out, b)?,
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(fields) => {
            let mut entries: Vec<_> = fields.iter().collect();
            entries.sort_unstable_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            out.push(b'{');
            for (index, (name, item)) in entries.into_iter().enumerate() {
                if name.len() > 4096 {
                    return Err(Error::Invalid("property name"));
                }
                if index > 0 {
                    out.push(b',');
                }
                serde_json::to_writer(&mut *out, name)?;
                out.push(b':');
                write(item, out)?;
            }
            out.push(b'}');
        }
        _ => return Err(Error::Invalid("unsupported JSON value")),
    }
    Ok(())
}
