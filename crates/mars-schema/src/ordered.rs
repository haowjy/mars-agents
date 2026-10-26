//! A JSON-shaped value that keeps mapping key order.
//!
//! The reading carries authored values (frontmatter, harness overrides, metadata) and the
//! emitter takes them back; both must preserve the writer's key order, which
//! `serde_json::Value` does not (and `serde_json/preserve_order` would change every JSON
//! output of the binary through feature unification).

use std::fmt;

use indexmap::IndexMap;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::Number;

/// Largest integer magnitude JavaScript represents unambiguously (`Number.MAX_SAFE_INTEGER`):
/// 2^53 is also what 2^53 + 1 rounds to.
pub(crate) const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

#[derive(Debug, Clone, PartialEq)]
pub enum OrderedValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<OrderedValue>),
    /// Keys in authored order. Equality ignores order; serialization does not.
    Object(IndexMap<String, OrderedValue>),
}

impl OrderedValue {
    /// The same value as a `serde_json::Value` (mapping keys then sort).
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(v) => (*v).into(),
            Self::Number(v) => serde_json::Value::Number(v.clone()),
            Self::String(v) => v.clone().into(),
            Self::Array(values) => values.iter().map(Self::to_json).collect(),
            Self::Object(map) => map
                .iter()
                .map(|(key, value)| (key.clone(), value.to_json()))
                .collect::<serde_json::Map<_, _>>()
                .into(),
        }
    }

    /// Convert YAML, refusing what JSON cannot hold: tags, non-string keys, and integers
    /// beyond ±(2^53 − 1).
    pub(crate) fn from_yaml(value: &serde_yaml::Value) -> Result<Self, String> {
        use serde_yaml::Value;
        Ok(match value {
            Value::Null => Self::Null,
            Value::Bool(v) => Self::Bool(*v),
            Value::Number(v) => {
                let beyond = v
                    .as_i64()
                    .is_some_and(|n| n.unsigned_abs() > MAX_SAFE_INTEGER)
                    || v.as_u64().is_some_and(|n| n > MAX_SAFE_INTEGER);
                if beyond {
                    return Err(format!(
                        "integer {v} is beyond ±(2^53 − 1), which JavaScript cannot hold exactly"
                    ));
                }
                Self::Number(
                    serde_json::to_value(v)
                        .map_err(|e| e.to_string())
                        .and_then(|n| match n {
                            serde_json::Value::Number(n) => Ok(n),
                            _ => Err("YAML number has no JSON form (NaN or infinity)".to_string()),
                        })?,
                )
            }
            Value::String(v) => Self::String(v.clone()),
            Value::Sequence(values) => Self::Array(
                values
                    .iter()
                    .map(Self::from_yaml)
                    .collect::<Result<_, _>>()?,
            ),
            Value::Mapping(values) => {
                let mut out = IndexMap::with_capacity(values.len());
                for (key, value) in values {
                    let Some(key) = key.as_str() else {
                        return Err("YAML mapping contains a non-string key".into());
                    };
                    reject_unholdable_key(key)?;
                    out.insert(key.to_owned(), Self::from_yaml(value)?);
                }
                Self::Object(out)
            }
            Value::Tagged(_) => return Err("YAML tags have no JSON representation".into()),
        })
    }
}

/// `__proto__` set on a plain JavaScript object changes its prototype instead of adding a key,
/// so a reading could not carry it.
pub(crate) fn reject_unholdable_key(key: &str) -> Result<(), String> {
    if key == "__proto__" {
        Err("key `__proto__` cannot be held as a plain property by JavaScript".into())
    } else {
        Ok(())
    }
}

impl Serialize for OrderedValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(v) => serializer.serialize_bool(*v),
            Self::Number(v) => v.serialize(serializer),
            Self::String(v) => serializer.serialize_str(v),
            Self::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    seq.serialize_element(value)?;
                }
                seq.end()
            }
            Self::Object(map) => {
                let mut out = serializer.serialize_map(Some(map.len()))?;
                for (key, value) in map {
                    out.serialize_entry(key, value)?;
                }
                out.end()
            }
        }
    }
}

/// Reads any JSON-shaped input in stream order. NaN and Infinity are refused rather than
/// turned into `null`.
impl<'de> Deserialize<'de> for OrderedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(OrderedVisitor)
    }
}

struct OrderedVisitor;

impl<'de> Visitor<'de> for OrderedVisitor {
    type Value = OrderedValue;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<OrderedValue, E> {
        Ok(OrderedValue::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<OrderedValue, E> {
        Ok(OrderedValue::Number(v.into()))
    }
    fn visit_u64<E>(self, v: u64) -> Result<OrderedValue, E> {
        Ok(OrderedValue::Number(v.into()))
    }
    fn visit_i128<E: de::Error>(self, v: i128) -> Result<OrderedValue, E> {
        i64::try_from(v)
            .map(|v| OrderedValue::Number(v.into()))
            .map_err(|_| E::custom("integer is beyond ±(2^53 − 1)"))
    }
    fn visit_u128<E: de::Error>(self, v: u128) -> Result<OrderedValue, E> {
        u64::try_from(v)
            .map(|v| OrderedValue::Number(v.into()))
            .map_err(|_| E::custom("integer is beyond ±(2^53 − 1)"))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<OrderedValue, E> {
        Number::from_f64(v)
            .map(OrderedValue::Number)
            .ok_or_else(|| E::custom("NaN and Infinity cannot be written as YAML"))
    }
    fn visit_str<E>(self, v: &str) -> Result<OrderedValue, E> {
        Ok(OrderedValue::String(v.to_owned()))
    }
    fn visit_unit<E>(self) -> Result<OrderedValue, E> {
        Ok(OrderedValue::Null)
    }
    fn visit_none<E>(self) -> Result<OrderedValue, E> {
        Ok(OrderedValue::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<OrderedValue, A::Error> {
        let mut out = Vec::new();
        while let Some(value) = seq.next_element()? {
            out.push(value);
        }
        Ok(OrderedValue::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<OrderedValue, A::Error> {
        let mut out = IndexMap::new();
        while let Some((key, value)) = map.next_entry::<String, OrderedValue>()? {
            out.insert(key, value);
        }
        Ok(OrderedValue::Object(out))
    }
}
