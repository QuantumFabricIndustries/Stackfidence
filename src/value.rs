//! `Value` — the typed payload that flows through the blackboard, edges, and
//! agent outputs.
//!
//! Agents and nodes communicate in `Value`s rather than raw `serde_json::Value`
//! so that the framework can:
//!   - validate shapes at agent boundaries (layer 11),
//!   - record exactly what was read/written for causal memory (layer 3),
//!   - keep working-set memory typed for decay decisions (layer 12).
//!
//! `Value` is intentionally a thin wrapper over `serde_json::Value` with a few
//! convenience constructors and a `shape()` helper used by the input validator.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::fmt;

#[derive(Clone, Debug, Eq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    FloatI(i64, u64), // (mantissa, scale) — represent common floats losslessly
    Str(String),
    /// A structured object: field name -> Value. Equality is order-independent
    /// (compared as a map), so two objects with the same fields in different
    /// order are equal.
    Object(Vec<(String, Value)>),
    /// An ordered list.
    List(Vec<Value>),
    /// An opaque blob carried verbatim (e.g. raw model output to be parsed
    /// later). Carries a mime hint.
    Blob(String, Vec<u8>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Object(a), Value::Object(b)) => {
                if a.len() != b.len() {
                    return false;
                }
                // Order-independent: every field in a must be in b with equal value.
                a.iter().all(|(k, va)| {
                    b.iter().any(|(kb, vb)| kb == k && vb == va)
                })
            }
            (l, r) => std::mem::discriminant(l) == std::mem::discriminant(r) && l.eq_ord(r),
        }
    }
}

impl Value {
    /// Order-dependent equality helper for non-object variants.
    fn eq_ord(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::FloatI(a, s), Value::FloatI(b, t)) => a == b && s == t,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Blob(m1, d1), Value::Blob(m2, d2)) => m1 == m2 && d1 == d2,
            (Value::List(a), Value::List(b)) => a == b,
            _ => false,
        }
    }
}

impl Value {
    pub fn null() -> Self {
        Value::Null
    }
    pub fn bool(b: bool) -> Self {
        Value::Bool(b)
    }
    pub fn int(i: i64) -> Self {
        Value::Int(i)
    }
    pub fn str<S: Into<String>>(s: S) -> Self {
        Value::Str(s.into())
    }
    pub fn obj<K: Into<String>, V: Into<Value>>(pairs: Vec<(K, V)>) -> Self {
        Value::Object(pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
    }
    pub fn list<V: Into<Value>>(items: Vec<V>) -> Self {
        Value::List(items.into_iter().map(|v| v.into()).collect())
    }

    /// Is this the null value?
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// A short structural signature used for boundary validation.
    ///
    /// e.g. `Object{count:Int,name:Str}`. This is *not* a full type system —
    /// it's a cheap shape check that catches malformed/missing fields without
    /// pulling in a schema library.
    pub fn shape(&self, depth: u8) -> String {
        match self {
            Value::Null => "Null".to_string(),
            Value::Bool(_) => "Bool".to_string(),
            Value::Int(_) => "Int".to_string(),
            Value::FloatI(_, _) => "Float".to_string(),
            Value::Str(_) => "Str".to_string(),
            Value::Blob(mime, _) => format!("Blob({})", mime),
            Value::List(items) => {
                if depth == 0 || items.is_empty() {
                    "List".to_string()
                } else {
                    format!("List[{}]", items[0].shape(depth - 1))
                }
            }
            Value::Object(fields) => {
                if depth == 0 || fields.is_empty() {
                    "Object".to_string()
                } else {
                    let inner: Vec<String> =
                        fields.iter().map(|(k, v)| format!("{}:{}", k, v.shape(depth - 1))).collect();
                    format!("Object{{{}}}", inner.join(","))
                }
            }
        }
    }

    /// Convert to a `serde_json::Value` for persistence / interop.
    pub fn to_json(&self) -> Json {
        match self {
            Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Int(i) => Json::from(*i),
            Value::FloatI(m, s) => {
                let f = if *s == 0 {
                    *m as f64
                } else {
                    *m as f64 / 10f64.powi(*s as i32)
                };
                serde_json::Number::from_f64(f)
                    .map(Json::Number)
                    .unwrap_or(Json::Null)
            }
            Value::Str(s) => Json::String(s.clone()),
            Value::Object(fields) => {
                let mut map = serde_json::Map::new();
                for (k, v) in fields {
                    map.insert(k.clone(), v.to_json());
                }
                Json::Object(map)
            }
            Value::List(items) => Json::Array(items.iter().map(|v| v.to_json()).collect()),
            Value::Blob(mime, bytes) => Json::Object({
                let mut m = serde_json::Map::new();
                m.insert("mime".into(), Json::String(mime.clone()));
                m.insert("bytes".into(), Json::Array(
                    bytes.iter().map(|b| Json::from(*b as i64)).collect(),
                ));
                m
            }),
        }
    }

    /// Best-effort parse from a `serde_json::Value`.
    pub fn from_json(j: &Json) -> Value {
        match j {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(*b),
            Json::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int(i)
                } else if let Some(u) = n.as_u64() {
                    Value::Int(u as i64)
                } else if let Some(f) = n.as_f64() {
                    // store as float with scale 0 (lossy but simple)
                    Value::FloatI(f as i64, 0)
                } else {
                    Value::Null
                }
            }
            Json::String(s) => Value::Str(s.clone()),
            Json::Array(items) => Value::List(items.iter().map(Value::from_json).collect()),
            Json::Object(map) => Value::Object(
                map.iter().map(|(k, v)| (k.clone(), Value::from_json(v))).collect(),
            ),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", serde_json::to_string(&self.to_json()).unwrap_or_default())
    }
}

// Convenience conversions.
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_of_object() {
        let v = Value::obj(vec![
            ("count", Value::int(3)),
            ("name", Value::str("agent")),
        ]);
        assert_eq!(v.shape(2), "Object{count:Int,name:Str}");
    }

    #[test]
    fn json_roundtrip() {
        let v = Value::obj(vec![("x", Value::int(1)), ("flag", Value::bool(true))]);
        let j = v.to_json();
        let back = Value::from_json(&j);
        assert_eq!(v, back);
    }

    #[test]
    fn null_is_null() {
        assert!(Value::null().is_null());
        assert!(!Value::int(0).is_null());
    }
}
