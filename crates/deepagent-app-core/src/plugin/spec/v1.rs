//! Agent Plugins v1 portable manifest parsing (spec §5).
//!
//! §5.2 makes the manifest schema **closed**: the only permitted top-level
//! fields are `$schema`, `name`, `version`, `description`, `author`,
//! `homepage`, `repository`, `license`, `keywords`, and `extensions`.
//!
//! Two violations are explicitly non-fatal and must be *reported and ignored*
//! rather than dropped silently:
//!
//! - an unknown top-level field (§5.2), and
//! - a non-object `extensions` (§8.1).
//!
//! Every other schema violation is fatal: the client must reject the plugin and
//! must not discover or execute any of its components.
//!
//! That asymmetry is why this module hand-walks a `serde_json::Value` instead of
//! deriving `Deserialize` with `deny_unknown_fields`. A derive can reject or
//! accept unknown fields, but it cannot accept-and-report them, and reporting is
//! what keeps a typo visible to the plugin author.
//!
//! §5.4 also requires *not* validating metadata beyond its JSON type: a client
//! must not reject a manifest merely because `version` is not SemVer, a URL is
//! unparseable, or `license` is not an SPDX identifier.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::plugin::model::PluginDiagnostic;
use crate::plugin::spec::name::{PluginName, PluginNameError};
use crate::plugin::spec::schema::{schema_status_of, SchemaStatus};

/// Top-level fields §5.2 permits. Anything else is an unknown field.
const PERMITTED_FIELDS: &[&str] = &[
    "$schema",
    "name",
    "version",
    "description",
    "author",
    "homepage",
    "repository",
    "license",
    "keywords",
    "extensions",
];

/// Fields §5.4 permits inside `author`. Any other member, or a non-string
/// value, makes the manifest invalid.
const PERMITTED_AUTHOR_FIELDS: &[&str] = &["name", "email", "url"];

/// The portable core of a plugin manifest (§5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableManifest {
    pub name: PluginName,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<PluginAuthor>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub keywords: Vec<String>,
    /// Client-specific data keyed by reverse-domain namespace (§8.1). Values are
    /// kept as raw JSON: §8.1 forbids validating namespaces we do not implement.
    pub extensions: BTreeMap<String, Value>,
}

/// Author metadata (§5.4). All three members are optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginAuthor {
    pub name: Option<String>,
    pub email: Option<String>,
    pub url: Option<String>,
}

/// A fatal manifest violation. §11.3 rule 2 requires rejecting the plugin
/// entirely for any of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortableManifestError {
    /// Not parseable as JSON.
    NotJson { reason: String },
    /// Parsed, but the document root is not an object (§5.2).
    NotObject { found: &'static str },
    /// `$schema` is absent (§5.3 makes it required).
    SchemaMissing,
    /// `$schema` targets an Agent Plugins version this build does not implement
    /// (§5.2). Reported with the value so the user learns which version.
    SchemaUnsupported { schema: String },
    /// `$schema` names something outside Agent Plugins entirely.
    SchemaUnrelated { schema: String },
    /// `name` is absent (§5.3).
    NameMissing,
    /// `name` violates the §5.5 constraints.
    NameInvalid { source: PluginNameError },
    /// A permitted field carried the wrong JSON type.
    FieldType {
        field: &'static str,
        expected: &'static str,
        found: &'static str,
    },
    /// `author` carried a member outside `name` / `email` / `url` (§5.4).
    AuthorUnknownField { field: String },
}

impl std::fmt::Display for PortableManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotJson { reason } => write!(f, "manifest is not valid JSON: {reason}"),
            Self::NotObject { found } => {
                write!(f, "manifest must be a JSON object, found {found}")
            }
            Self::SchemaMissing => write!(f, "manifest is missing the required `$schema` field"),
            Self::SchemaUnsupported { schema } => write!(
                f,
                "unsupported Agent Plugins version declared by `$schema`: {schema}"
            ),
            Self::SchemaUnrelated { schema } => write!(
                f,
                "`$schema` does not identify an Agent Plugins manifest: {schema}"
            ),
            Self::NameMissing => write!(f, "manifest is missing the required `name` field"),
            Self::NameInvalid { source } => write!(f, "invalid plugin name: {source}"),
            Self::FieldType {
                field,
                expected,
                found,
            } => write!(f, "`{field}` must be {expected}, found {found}"),
            Self::AuthorUnknownField { field } => write!(
                f,
                "`author` may only contain `name`, `email`, and `url`; found `{field}`"
            ),
        }
    }
}

impl std::error::Error for PortableManifestError {}

/// Parses a portable `plugin.json`.
///
/// Returns the manifest plus the non-fatal findings §5.2 and §8.1 require to be
/// reported. A fatal violation yields `Err` and the caller must not discover any
/// component of that plugin.
pub fn parse_portable(
    contents: &str,
) -> Result<(PortableManifest, Vec<PluginDiagnostic>), PortableManifestError> {
    let value: Value =
        serde_json::from_str(contents).map_err(|error| PortableManifestError::NotJson {
            reason: error.to_string(),
        })?;
    let Value::Object(object) = value else {
        return Err(PortableManifestError::NotObject {
            found: json_type(&value),
        });
    };

    let mut diagnostics = Vec::new();

    // §5.2: report and ignore unknown top-level fields, then keep loading.
    for field in object.keys() {
        if !PERMITTED_FIELDS.contains(&field.as_str()) {
            diagnostics.push(PluginDiagnostic::UnknownManifestField {
                field: field.clone(),
            });
        }
    }

    // §5.2/§5.3: `$schema` is required and selects the interpretation rules.
    let schema =
        required_string(&object, "$schema")?.ok_or(PortableManifestError::SchemaMissing)?;
    match schema_status_of(Some(schema.as_str())) {
        SchemaStatus::Supported => {}
        SchemaStatus::Unsupported => {
            return Err(PortableManifestError::SchemaUnsupported { schema })
        }
        SchemaStatus::Unrelated => return Err(PortableManifestError::SchemaUnrelated { schema }),
    }

    let raw_name = required_string(&object, "name")?.ok_or(PortableManifestError::NameMissing)?;
    let name = PluginName::parse(&raw_name)
        .map_err(|source| PortableManifestError::NameInvalid { source })?;

    let manifest = PortableManifest {
        name,
        version: required_string(&object, "version")?,
        description: required_string(&object, "description")?,
        author: parse_author(&object)?,
        homepage: required_string(&object, "homepage")?,
        repository: required_string(&object, "repository")?,
        license: required_string(&object, "license")?,
        keywords: parse_keywords(&object)?,
        extensions: parse_extensions(&object, &mut diagnostics),
    };

    Ok((manifest, diagnostics))
}

/// Reads an optional string field, treating `null` as absent and any other
/// non-string as a fatal type violation.
fn required_string(
    object: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, PortableManifestError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Err(PortableManifestError::FieldType {
            field,
            expected: "a string",
            found: json_type(other),
        }),
    }
}

fn parse_author(
    object: &serde_json::Map<String, Value>,
) -> Result<Option<PluginAuthor>, PortableManifestError> {
    let value = match object.get("author") {
        None | Some(Value::Null) => return Ok(None),
        Some(value) => value,
    };
    let Value::Object(author) = value else {
        return Err(PortableManifestError::FieldType {
            field: "author",
            expected: "an object",
            found: json_type(value),
        });
    };

    // §5.4: the author object is closed, and an unexpected member is fatal —
    // unlike an unknown *top-level* field, which §5.2 downgrades to a report.
    for field in author.keys() {
        if !PERMITTED_AUTHOR_FIELDS.contains(&field.as_str()) {
            return Err(PortableManifestError::AuthorUnknownField {
                field: field.clone(),
            });
        }
    }

    Ok(Some(PluginAuthor {
        name: author_string(author, "name")?,
        email: author_string(author, "email")?,
        url: author_string(author, "url")?,
    }))
}

fn author_string(
    author: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, PortableManifestError> {
    match author.get(field) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        // §5.4 spells out that any non-string author value invalidates the
        // manifest, so `null` is not accepted as "absent" here.
        Some(other) => Err(PortableManifestError::FieldType {
            field,
            expected: "a string",
            found: json_type(other),
        }),
    }
}

fn parse_keywords(
    object: &serde_json::Map<String, Value>,
) -> Result<Vec<String>, PortableManifestError> {
    let value = match object.get("keywords") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(value) => value,
    };
    let Value::Array(items) = value else {
        return Err(PortableManifestError::FieldType {
            field: "keywords",
            expected: "an array of strings",
            found: json_type(value),
        });
    };

    items
        .iter()
        .map(|item| match item {
            Value::String(keyword) => Ok(keyword.clone()),
            other => Err(PortableManifestError::FieldType {
                field: "keywords",
                expected: "an array of strings",
                found: json_type(other),
            }),
        })
        .collect()
}

/// §8.1: `extensions` must be an object of objects. A non-object `extensions`
/// is reported and ignored; a non-object *member* is dropped the same way,
/// because validating a namespace we may not implement is forbidden and an
/// unusable value is better skipped than trusted.
fn parse_extensions(
    object: &serde_json::Map<String, Value>,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> BTreeMap<String, Value> {
    let Some(value) = object.get("extensions") else {
        return BTreeMap::new();
    };
    let Value::Object(extensions) = value else {
        diagnostics.push(PluginDiagnostic::ExtensionsNotObject);
        return BTreeMap::new();
    };

    let mut out = BTreeMap::new();
    for (namespace, data) in extensions {
        if data.is_object() {
            out.insert(namespace.clone(), data.clone());
        } else {
            diagnostics.push(PluginDiagnostic::UnknownManifestField {
                field: format!("extensions.{namespace}"),
            });
        }
    }
    out
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::spec::schema::AGENT_PLUGIN_SCHEMA_URI;

    fn manifest(body: &str) -> String {
        format!(r#"{{"$schema": "{AGENT_PLUGIN_SCHEMA_URI}", {body}}}"#)
    }

    /// The minimal manifest from §5.2.
    #[test]
    fn accepts_spec_minimal_manifest() {
        let (parsed, diagnostics) =
            parse_portable(&manifest(r#""name": "minimal-plugin""#)).expect("valid");

        assert_eq!(parsed.name.as_str(), "minimal-plugin");
        assert_eq!(parsed.version, None);
        assert!(parsed.keywords.is_empty());
        assert!(parsed.extensions.is_empty());
        assert!(diagnostics.is_empty());
    }

    /// The full manifest from §5.2, every permitted field populated.
    #[test]
    fn accepts_spec_full_manifest() {
        let contents = manifest(
            r#""name": "plugin-name",
               "version": "1.2.0",
               "description": "Brief plugin description",
               "author": {
                 "name": "Author Name",
                 "email": "author@example.com",
                 "url": "https://example.com"
               },
               "homepage": "https://docs.example.com/plugin",
               "repository": "https://github.com/example/plugin",
               "license": "MIT",
               "keywords": ["keyword1", "keyword2"],
               "extensions": { "com.example.client": { "setting": true } }"#,
        );

        let (parsed, diagnostics) = parse_portable(&contents).expect("valid");

        assert_eq!(parsed.name.as_str(), "plugin-name");
        assert_eq!(parsed.version.as_deref(), Some("1.2.0"));
        assert_eq!(
            parsed.description.as_deref(),
            Some("Brief plugin description")
        );
        assert_eq!(parsed.license.as_deref(), Some("MIT"));
        assert_eq!(parsed.keywords, vec!["keyword1", "keyword2"]);
        let author = parsed.author.expect("author");
        assert_eq!(author.name.as_deref(), Some("Author Name"));
        assert_eq!(author.email.as_deref(), Some("author@example.com"));
        assert_eq!(author.url.as_deref(), Some("https://example.com"));
        assert!(parsed.extensions.contains_key("com.example.client"));
        assert!(diagnostics.is_empty());
    }

    /// §5.2: an unknown top-level field is reported and ignored, and the plugin
    /// still loads.
    #[test]
    fn unknown_top_level_field_is_reported_not_fatal() {
        let contents = manifest(r#""name": "demo", "mcpServers": { "db": {} }"#);

        let (parsed, diagnostics) = parse_portable(&contents).expect("non-fatal");

        assert_eq!(parsed.name.as_str(), "demo");
        assert_eq!(
            diagnostics,
            vec![PluginDiagnostic::UnknownManifestField {
                field: "mcpServers".to_string()
            }]
        );
    }

    /// §8.1: a non-object `extensions` is reported and ignored, components keep
    /// loading.
    #[test]
    fn non_object_extensions_is_reported_not_fatal() {
        let contents = manifest(r#""name": "demo", "extensions": ["nope"]"#);

        let (parsed, diagnostics) = parse_portable(&contents).expect("non-fatal");

        assert!(parsed.extensions.is_empty());
        assert!(diagnostics.contains(&PluginDiagnostic::ExtensionsNotObject));
    }

    #[test]
    fn non_object_extension_member_is_dropped_with_a_report() {
        let contents = manifest(r#""name": "demo", "extensions": { "com.example": 7 }"#);

        let (parsed, diagnostics) = parse_portable(&contents).expect("non-fatal");

        assert!(parsed.extensions.is_empty());
        assert!(
            diagnostics.contains(&PluginDiagnostic::UnknownManifestField {
                field: "extensions.com.example".to_string()
            })
        );
    }

    /// §8.1: values of namespaces we do not implement must not be validated,
    /// so arbitrary nested JSON survives untouched.
    #[test]
    fn unimplemented_extension_values_pass_through_unvalidated() {
        let contents =
            manifest(r#""name": "demo", "extensions": { "com.other.client": { "a": [1, null] } }"#);

        let (parsed, _) = parse_portable(&contents).expect("valid");

        assert_eq!(
            parsed.extensions["com.other.client"],
            serde_json::json!({ "a": [1, null] })
        );
    }

    #[test]
    fn rejects_missing_schema() {
        let err = parse_portable(r#"{"name": "demo"}"#).expect_err("fatal");
        assert_eq!(err, PortableManifestError::SchemaMissing);
    }

    /// §5.2: an Agent Plugins version we do not implement must be rejected and
    /// the version reported.
    #[test]
    fn rejects_unsupported_schema_version() {
        let contents = r#"{"$schema": "https://agent-plugins.org/schemas/2.0.0/plugin.schema.json",
                "name": "demo"}"#;
        let err = parse_portable(contents).expect_err("fatal");
        assert!(matches!(
            err,
            PortableManifestError::SchemaUnsupported { .. }
        ));
        assert!(err.to_string().contains("2.0.0"));
    }

    #[test]
    fn rejects_foreign_schema() {
        let contents = r#"{"$schema": "https://json.schemastore.org/package.json",
                          "name": "demo"}"#;
        assert!(matches!(
            parse_portable(contents),
            Err(PortableManifestError::SchemaUnrelated { .. })
        ));
    }

    #[test]
    fn rejects_missing_or_invalid_name() {
        assert_eq!(
            parse_portable(&manifest(r#""version": "1.0.0""#)).expect_err("fatal"),
            PortableManifestError::NameMissing
        );
        assert!(matches!(
            parse_portable(&manifest(r#""name": "Bad-Name""#)),
            Err(PortableManifestError::NameInvalid { .. })
        ));
        assert!(matches!(
            parse_portable(&manifest(r#""name": 7"#)),
            Err(PortableManifestError::FieldType { field: "name", .. })
        ));
    }

    #[test]
    fn rejects_non_object_document() {
        assert!(matches!(
            parse_portable("[]"),
            Err(PortableManifestError::NotObject { found: "an array" })
        ));
        assert!(matches!(
            parse_portable("\"text\""),
            Err(PortableManifestError::NotObject { found: "a string" })
        ));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(
            parse_portable("{not json"),
            Err(PortableManifestError::NotJson { .. })
        ));
    }

    /// §5.4: the author object is closed and an extra member is fatal — this is
    /// the deliberate asymmetry against unknown top-level fields.
    #[test]
    fn rejects_author_with_extra_member() {
        let contents = manifest(r#""name": "demo", "author": { "name": "A", "twitter": "@a" }"#);
        assert_eq!(
            parse_portable(&contents).expect_err("fatal"),
            PortableManifestError::AuthorUnknownField {
                field: "twitter".to_string()
            }
        );
    }

    #[test]
    fn rejects_non_string_author_values() {
        let contents = manifest(r#""name": "demo", "author": { "name": 7 }"#);
        assert!(matches!(
            parse_portable(&contents),
            Err(PortableManifestError::FieldType { field: "name", .. })
        ));

        let contents = manifest(r#""name": "demo", "author": "Author Name""#);
        assert!(matches!(
            parse_portable(&contents),
            Err(PortableManifestError::FieldType {
                field: "author",
                ..
            })
        ));
    }

    /// §5.4: metadata is validated only by JSON type. None of these shapes may
    /// be rejected, however wrong they look.
    #[test]
    fn accepts_metadata_that_fails_semantic_conventions() {
        let contents = manifest(
            r#""name": "demo",
               "version": "not-semver",
               "homepage": "not a url",
               "repository": "also not a url",
               "license": "Definitely-Not-SPDX",
               "author": { "email": "not-an-email", "url": "nope" }"#,
        );

        let (parsed, diagnostics) = parse_portable(&contents).expect("must not be rejected");

        assert_eq!(parsed.version.as_deref(), Some("not-semver"));
        assert_eq!(parsed.license.as_deref(), Some("Definitely-Not-SPDX"));
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn rejects_wrong_metadata_types() {
        for (field, body) in [
            ("version", r#""name": "demo", "version": 1.2"#),
            ("description", r#""name": "demo", "description": []"#),
            ("homepage", r#""name": "demo", "homepage": true"#),
            ("keywords", r#""name": "demo", "keywords": "single""#),
            ("keywords", r#""name": "demo", "keywords": ["ok", 7]"#),
        ] {
            let err = parse_portable(&manifest(body)).expect_err("fatal");
            assert!(
                matches!(err, PortableManifestError::FieldType { field: f, .. } if f == field),
                "expected {field} type error, got {err:?}"
            );
        }
    }

    /// `null` reads as absent for optional strings, which keeps a manifest that
    /// serialized empty values from being rejected outright.
    #[test]
    fn null_optional_fields_read_as_absent() {
        let contents = manifest(r#""name": "demo", "version": null, "keywords": null"#);

        let (parsed, _) = parse_portable(&contents).expect("valid");

        assert_eq!(parsed.version, None);
        assert!(parsed.keywords.is_empty());
    }

    #[test]
    fn reports_every_unknown_field() {
        let contents = manifest(r#""name": "demo", "skills": "./skills", "hooks": "./h.json""#);

        let (_, diagnostics) = parse_portable(&contents).expect("non-fatal");

        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics.contains(&PluginDiagnostic::UnknownManifestField {
                field: "skills".to_string()
            })
        );
        assert!(
            diagnostics.contains(&PluginDiagnostic::UnknownManifestField {
                field: "hooks".to_string()
            })
        );
    }

    /// A fatal violation must win over a report: §11.3 forbids discovering
    /// components of a rejected plugin, so the diagnostics are irrelevant then.
    #[test]
    fn fatal_violation_wins_over_unknown_field_report() {
        let contents = manifest(r#""name": "Bad-Name", "unknownField": 1"#);
        assert!(parse_portable(&contents).is_err());
    }
}
