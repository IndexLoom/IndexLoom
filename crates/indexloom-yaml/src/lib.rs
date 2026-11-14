//! Safe YAML parsing for indexer definitions and Cardigann values.
//! Mapping order is retained because Cardigann field evaluation is ordered.

pub use serde_json::Value;
pub use serde_saphyr::{Error, to_string};
pub type Mapping = serde_json::Map<String, Value>;

/// Parse a single YAML document, rejecting duplicate keys at every depth.
///
/// # Errors
/// Returns an error for malformed YAML, duplicate keys, or exceeded parser budgets.
pub fn from_str<'de, T: serde::Deserialize<'de>>(source: &'de str) -> Result<T, Error> {
    // Match the catalog's existing per-definition limit for direct callers too.
    if source.len() > 2 * 1024 * 1024 {
        return Err(<Error as serde::de::Error>::custom(
            "YAML input exceeds 2 MiB",
        ));
    }
    serde_saphyr::from_str_with_options(
        source,
        serde_saphyr::options! {
            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
            strict_booleans: true,
            leading_zero_numbers_as_strings: true,
            underscored_numbers_as_strings: true,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{Value, from_str, to_string};
    use serde_json::json;

    #[test]
    fn yaml_scalars_keep_null_boolean_number_and_string_semantics() {
        let value: Value = from_str(
            r#"
empty:
nulls: [null, Null, NULL, ~]
strings: ["null", "", 'true', yes, no, on, off, 0755, 1_000]
numbers: [42, -7, 1.5, 0xFF]
booleans: [true, FALSE]
inputs: {q: '{{ .Keywords }}', missing: null}
"#,
        )
        .unwrap();
        assert_eq!(value["empty"], Value::Null);
        assert_eq!(value["nulls"], json!([null, null, null, null]));
        assert_eq!(
            value["strings"],
            json!([
                "null", "", "true", "yes", "no", "on", "off", "0755", "1_000"
            ])
        );
        assert_eq!(value["numbers"], json!([42, -7, 1.5, 255]));
        assert_eq!(value["booleans"], json!([true, false]));
        assert_eq!(value["inputs"]["q"], "{{ .Keywords }}");
        assert_eq!(value["inputs"]["missing"], Value::Null);
        assert_eq!(
            from_str::<Value>(&to_string(&value).unwrap()).unwrap(),
            value
        );
    }

    #[test]
    fn json_documents_round_trip_through_yaml() {
        let value: Value = from_str(
            r#"{"null":null,"bool":true,"int":17,"float":1.25,"list":["a"],"map":{"b":"c"}}"#,
        )
        .unwrap();
        assert_eq!(
            value,
            json!({"null":null,"bool":true,"int":17,"float":1.25,"list":["a"],"map":{"b":"c"}})
        );
        assert_eq!(
            from_str::<Value>(&to_string(&value).unwrap()).unwrap(),
            value
        );
    }

    #[test]
    fn duplicate_keys_and_malformed_documents_are_rejected() {
        for yaml in [
            "a: 1\na: 2",
            "outer: {a: 1, a: 2}",
            "items: [{a: 1, a: 2}]",
            "a: [1, 2",
            "a: 1\n---\nb: 2",
            "a: *missing",
            "a: .nan",
            "a: .inf",
        ] {
            assert!(from_str::<Value>(yaml).is_err(), "accepted {yaml:?}");
        }
    }

    #[test]
    fn parser_budgets_reject_excessive_nesting() {
        let source = format!("{}0{}", "[".repeat(1024), "]".repeat(1024));
        assert!(from_str::<Value>(&source).is_err());
        assert!(from_str::<Value>(&"x".repeat(2 * 1024 * 1024 + 1)).is_err());
    }

    #[test]
    fn catalog_fixtures_preserve_order_templates_and_numeric_category_keys() {
        for source in [
            include_str!("../../indexloom-engine/tests/fixtures/interactive.yml"),
            include_str!("../../indexloom-engine/tests/fixtures/cardigann/html.yml"),
            include_str!("../../indexloom-engine/tests/fixtures/cardigann/json.yml"),
            include_str!("../../indexloom-engine/tests/fixtures/cardigann/xml.yml"),
            include_str!("../../indexloom-engine/tests/fixtures/cardigann/filters.yml"),
            include_str!("../../indexloom-engine/src/cardigann/download/fixtures/indexer.yml"),
        ] {
            let value: Value = from_str(source).unwrap();
            assert!(value.is_object() || value.is_array());
            assert_eq!(
                from_str::<Value>(&to_string(&value).unwrap()).unwrap(),
                value
            );
        }
        let map: Value =
            from_str("fields: {z: first, a: second}\ncategories: {2000: [1, 2]}").unwrap();
        assert_eq!(
            map["fields"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["z", "a"]
        );
        assert_eq!(map["categories"]["2000"], json!([1, 2]));
    }
}
