//! Structured conditions used by `show_if` / `required_if` in the metadata files.
//!
//! A condition is a plain object, so the TypeScript side can evaluate the same JSON:
//!
//! ```text
//! { key, eq: V }          resolved value of `key` equals V
//! { key, neq: V }         resolved value of `key` differs from V
//! { key, in: [V, ...] }   resolved value of `key` is one of the listed values
//! { all: [cond, ...] }    every sub-condition holds
//! { any: [cond, ...] }    at least one sub-condition holds
//! ```
//!
//! `key` is an absolute dotted config path. A key that resolves to nothing behaves as `null`:
//! `eq` and `in` are false, `neq` is true. Numbers compare by value (`6` equals `6.0`).
//!
//! Shared test vectors live in `meta/condition-vectors.json` and must pass in every
//! implementation (Rust here, TypeScript in the web frontend).

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Source of resolved config values, keyed by absolute dotted path.
///
/// Implementations return the value *after* default resolution so an unset key behaves like its
/// default (e.g. an unset `input.rate_mode` resolves to `"dynamic"`).
pub trait Resolver {
    fn resolve(&self, path: &str) -> Option<Value>;
}

impl Resolver for BTreeMap<String, Value> {
    fn resolve(&self, path: &str) -> Option<Value> {
        self.get(path).cloned()
    }
}

/// A visibility / requirement condition.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Condition {
    Eq {
        key: String,
        eq: Value,
    },
    Neq {
        key: String,
        neq: Value,
    },
    In {
        key: String,
        #[serde(rename = "in")]
        values: Vec<Value>,
    },
    All {
        all: Vec<Condition>,
    },
    Any {
        any: Vec<Condition>,
    },
}

/// Loose shape used only to give precise errors; exactly one operator must be present.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    key: Option<String>,
    eq: Option<Value>,
    neq: Option<Value>,
    #[serde(rename = "in")]
    values: Option<Vec<Value>>,
    all: Option<Vec<Condition>>,
    any: Option<Vec<Condition>>,
}

impl TryFrom<Raw> for Condition {
    type Error = String;

    fn try_from(r: Raw) -> Result<Self, String> {
        let ops = [
            r.eq.is_some(),
            r.neq.is_some(),
            r.values.is_some(),
            r.all.is_some(),
            r.any.is_some(),
        ]
        .into_iter()
        .filter(|&b| b)
        .count();
        if ops != 1 {
            return Err(format!(
                "a condition needs exactly one of eq, neq, in, all, any (found {ops})"
            ));
        }
        let leaf = r.eq.is_some() || r.neq.is_some() || r.values.is_some();
        if leaf != r.key.is_some() {
            return Err("`key` goes with eq / neq / in, and only with them".to_string());
        }
        Ok(match (r.key, r.eq, r.neq, r.values, r.all, r.any) {
            (Some(key), Some(eq), ..) => Self::Eq { key, eq },
            (Some(key), _, Some(neq), ..) => Self::Neq { key, neq },
            (Some(key), _, _, Some(values), ..) => {
                if values.is_empty() {
                    return Err("`in` needs at least one value".to_string());
                }
                Self::In { key, values }
            }
            (_, _, _, _, Some(all), _) => {
                if all.is_empty() {
                    return Err("`all` needs at least one condition".to_string());
                }
                Self::All { all }
            }
            (_, _, _, _, _, Some(any)) => {
                if any.is_empty() {
                    return Err("`any` needs at least one condition".to_string());
                }
                Self::Any { any }
            }
            _ => unreachable!("operator count checked above"),
        })
    }
}

impl<'de> Deserialize<'de> for Condition {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Raw::deserialize(d)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

impl Condition {
    /// Evaluates against resolved config values.
    pub fn eval(&self, r: &dyn Resolver) -> bool {
        match self {
            Self::Eq { key, eq } => r.resolve(key).is_some_and(|v| same(&v, eq)),
            Self::Neq { key, neq } => !r.resolve(key).is_some_and(|v| same(&v, neq)),
            Self::In { key, values } => r
                .resolve(key)
                .is_some_and(|v| values.iter().any(|x| same(&v, x))),
            Self::All { all } => all.iter().all(|c| c.eval(r)),
            Self::Any { any } => any.iter().any(|c| c.eval(r)),
        }
    }

    /// Every `(key, values compared against)` pair in the condition.
    pub fn comparisons(&self) -> Vec<(&str, Vec<&Value>)> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<(&'a str, Vec<&'a Value>)>) {
        match self {
            Self::Eq { key, eq } => out.push((key, vec![eq])),
            Self::Neq { key, neq } => out.push((key, vec![neq])),
            Self::In { key, values } => out.push((key, values.iter().collect())),
            Self::All { all: cs } | Self::Any { any: cs } => {
                cs.iter().for_each(|c| c.collect(out));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Deserialize)]
    struct Vector {
        name: String,
        condition: Value,
        values: BTreeMap<String, Value>,
        expected: bool,
    }

    #[test]
    fn shared_vectors_pass() {
        let vectors: Vec<Vector> =
            serde_json::from_str(include_str!("../../meta/condition-vectors.json")).unwrap();
        assert!(vectors.len() >= 10);
        for v in vectors {
            let c: Condition = serde_json::from_value(v.condition)
                .unwrap_or_else(|e| panic!("vector `{}` failed to parse: {e}", v.name));
            assert_eq!(c.eval(&v.values), v.expected, "vector `{}`", v.name);
        }
    }

    #[test]
    fn rejects_malformed_conditions() {
        for bad in [
            json!({}),
            json!({"key": "a.b"}),
            json!({"eq": 1}),
            json!({"key": "a.b", "eq": 1, "neq": 2}),
            json!({"key": "a.b", "in": []}),
            json!({"key": "a.b", "all": []}),
            json!({"all": []}),
            json!({"any": []}),
            json!({"key": "a.b", "eq": 1, "extra": true}),
            json!({"all": [{"key": "a.b"}]}),
            json!("a.b == 1"),
        ] {
            assert!(
                serde_json::from_value::<Condition>(bad.clone()).is_err(),
                "should reject {bad}"
            );
        }
    }

    #[test]
    fn toml_form_and_round_trip() {
        #[derive(Deserialize)]
        struct W {
            show_if: Condition,
        }
        let w: W = toml::from_str(
            r#"show_if = { all = [ { key = "a.b", eq = "x" }, { key = "c.d", in = [1, 2] } ] }"#,
        )
        .unwrap();
        let back: Condition =
            serde_json::from_value(serde_json::to_value(&w.show_if).unwrap()).unwrap();
        assert_eq!(back, w.show_if);
        let c = w.show_if.comparisons();
        assert_eq!(c[0].0, "a.b");
        assert_eq!(c[1].1.len(), 2);
    }
}
