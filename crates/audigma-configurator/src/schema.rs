//! JSON Schema for the configuration and for the metadata served to frontends.
//!
//! The Rust types are the single source of truth. [`generated_files`] renders the committed
//! `web/src/generated/*.schema.json` files; `examples/gen_schema.rs` writes them (and runs the
//! TypeScript / zod generators) and `tests/schema_drift.rs` fails when the committed copies are
//! stale.
//!
//! Descriptions are stripped from the schemas: the prose lives in `meta/*.toml`, and rustdoc is
//! developer-facing.

use audigma_settings::Settings;
use audigma_settings::meta::MetaBundle;
use schemars::generate::SchemaSettings;
use schemars::transform::{Transform, transform_subschemas};
use schemars::{JsonSchema, Schema};
use serde_json::Value;

/// Removes `description` and `title` from every (sub)schema, and turns `oneOf` lists of string
/// constants (serde enums whose variants had doc comments) into a plain `enum`, which the
/// TypeScript and zod generators render far better.
#[derive(Clone)]
struct StripProse;

fn string_consts(schema: &Schema) -> Option<Vec<Value>> {
    let arms = schema.get("oneOf")?.as_array()?;
    let mut values = Vec::new();
    for arm in arms {
        let o = arm.as_object()?;
        if o.len() != 2 || o.get("type")? != "string" {
            return None;
        }
        // schemars writes one-value variants as `const`, or as a one-element `enum`.
        match (
            o.get("const"),
            o.get("enum").and_then(Value::as_array).map(Vec::as_slice),
        ) {
            (Some(c), None) => values.push(c.clone()),
            (None, Some([only])) => values.push(only.clone()),
            _ => return None,
        }
    }
    Some(values)
}

impl Transform for StripProse {
    fn transform(&mut self, schema: &mut Schema) {
        schema.remove("description");
        schema.remove("title");
        // Children first: their descriptions must be gone before the arms are inspected.
        transform_subschemas(self, schema);
        if let Some(values) = string_consts(schema) {
            schema.remove("oneOf");
            schema.insert("type".into(), "string".into());
            schema.insert("enum".into(), Value::Array(values));
        }
    }
}

fn render<T: JsonSchema>(settings: SchemaSettings) -> String {
    let generator = settings.with_transform(StripProse).into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let mut text = serde_json::to_string_pretty(&schema).expect("schema is valid JSON");
    text.push('\n');
    text
}

/// Schema of the configuration file (what a user may write). Deserialization view.
pub fn settings_schema() -> String {
    render::<Settings>(SchemaSettings::draft2020_12())
}

/// Schema of `GET /api/meta`. Serialization view (the server only produces it).
pub fn meta_schema() -> String {
    render::<MetaBundle>(SchemaSettings::draft2020_12().for_serialize())
}

/// Metadata as JSON, in the shape [`meta_schema`] describes.
pub fn meta_json() -> Value {
    let catalog = audigma_settings::meta::Catalog::load().expect("embedded metadata must load");
    serde_json::to_value(catalog.bundle()).expect("bundle serializes")
}

/// `(file name under web/src/generated, contents)` for every schema file.
pub fn generated_files() -> Vec<(&'static str, String)> {
    vec![
        ("settings.schema.json", settings_schema()),
        ("meta.schema.json", meta_schema()),
    ]
}
