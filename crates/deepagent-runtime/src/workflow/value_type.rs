//! One v1 value-type authority; each boundary owns its presence/null policy.
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ValueType {
    String,
    Number,
    Boolean,
    Object,
    Array,
    File,
}

impl ValueType {
    pub(super) fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "string" => Self::String,
            "number" => Self::Number,
            "boolean" => Self::Boolean,
            "object" => Self::Object,
            "array" => Self::Array,
            "file" => Self::File,
            _ => return None,
        })
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Object => "object",
            Self::Array => "array",
            Self::File => "file",
        }
    }

    pub(super) fn accepts(self, value: &Value) -> bool {
        match (self, value) {
            (Self::String, Value::String(_))
            | (Self::Number, Value::Number(_))
            | (Self::Boolean, Value::Bool(_))
            | (Self::Object, Value::Object(_))
            | (Self::Array, Value::Array(_)) => true,
            // An opaque reference, never a host path or an artifact access grant.
            (Self::File, Value::String(reference)) => {
                reference.starts_with("artifact://") && reference.len() > "artifact://".len()
            }
            _ => false,
        }
    }

    pub(super) fn actual_name(value: &Value) -> &'static str {
        match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn h197_all_six_types_use_exact_json_types_without_coercion_or_null() {
        let cases = [
            ("string", json!("")),
            ("number", json!(0)),
            ("boolean", json!(false)),
            ("object", json!({"nested":[null,false]})),
            ("array", json!([{}, null])),
            ("file", json!("artifact://synthetic")),
        ];
        for (name, value) in &cases {
            let kind = ValueType::parse(name).unwrap();
            assert_eq!(kind.name(), *name);
            assert!(kind.accepts(value));
            assert!(!kind.accepts(&Value::Null));
            for (other, value) in &cases {
                assert_eq!(
                    kind.accepts(value),
                    name == other || (*name == "string" && *other == "file")
                );
            }
        }
        for name in ["", "null", "integer", "String", "number "] {
            assert!(ValueType::parse(name).is_none());
        }
        for value in [
            json!("artifact://"),
            json!("C:\\synthetic.txt"),
            json!("https://fixture.invalid"),
            json!({"uri":"artifact://synthetic"}),
        ] {
            assert!(!ValueType::File.accepts(&value));
        }
        assert!(ValueType::Number.accepts(&json!(9_007_199_254_740_993_u64)));
        assert!(ValueType::Number.accepts(&json!(-0.5)));
    }
}
