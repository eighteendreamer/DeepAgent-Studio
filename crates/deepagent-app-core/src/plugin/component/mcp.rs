//! MCP server discovery from the fixed `mcp.json` location (spec §7.2).
//!
//! §7.2.1 defines a **closed union**: each server declares a `type` and must
//! match exactly one variant. An unknown field, an unknown `type`, or a field
//! belonging to another variant invalidates that server entry.
//!
//! §7.2.2 defines two independent failure scopes, and conflating them is the
//! easiest way to get this wrong:
//!
//! - A problem with the *file* (unparseable, extra top-level field, schema
//!   version mismatch) disables MCP for the plugin while every other component
//!   type keeps loading.
//! - A problem with *one server entry* skips that entry only; its siblings and
//!   other component types keep loading.
//!
//! # Known gap: `cwd` has no home in the shared MCP config yet
//!
//! `deepagent_mcp::config::McpServerConfig` carries transport, command, args,
//! env, url, and headers — but no working directory. v1 stdio servers may set
//! `cwd`, and §7.2.1 also makes the plugin root the *default* working directory.
//! So this module keeps `cwd` in its own [`PluginMcpServer`] representation
//! rather than mapping to the shared config immediately. Wiring it up (either by
//! adding a field to the shared config or by setting the working directory where
//! the process is spawned) belongs to the runtime projection step, and dropping
//! `cwd` there would silently break conformant plugins.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::plugin::model::PluginDiagnostic;
use crate::plugin::spec::placeholder::{
    classify_cwd, normalize_and_expand, reserved_env_key, CwdForm,
};
use crate::plugin::spec::schema::{
    mcp_schema_status, read_schema_value, versions_match, SchemaStatus,
    AGENT_PLUGIN_MCP_RELATIVE_PATH,
};
use crate::plugin::spec::{is_within, resolve_plugin_relative};

/// Top-level fields §7.2.1 permits in `mcp.json`.
const PERMITTED_TOP_LEVEL: &[&str] = &["$schema", "mcpServers"];

/// Fields permitted on a `stdio` server (§7.2.1).
const STDIO_FIELDS: &[&str] = &["type", "command", "args", "env", "cwd"];

/// Fields permitted on a remote server, both `streamable-http` and `sse`.
const REMOTE_FIELDS: &[&str] = &["type", "url", "headers"];

/// Whether the plugin declares MCP servers, and if not, why (§7.2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    /// No `mcp.json` at the plugin root. Not an error (§6.2).
    Absent,
    /// Present but unusable as a whole. Other component types keep loading.
    Disabled { reason: String },
    /// Loaded; `servers` holds the entries that passed validation.
    Loaded,
}

/// The MCP component of one plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpComponent {
    pub status: McpStatus,
    /// Only entries that passed validation. Skipped entries appear as
    /// diagnostics, never here.
    pub servers: Vec<PluginMcpServer>,
}

impl McpComponent {
    fn absent() -> Self {
        Self {
            status: McpStatus::Absent,
            servers: Vec::new(),
        }
    }

    fn disabled(reason: impl Into<String>) -> Self {
        Self {
            status: McpStatus::Disabled {
                reason: reason.into(),
            },
            servers: Vec::new(),
        }
    }
}

/// A validated MCP server declared by a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMcpServer {
    /// The `mcpServers` member name identifying this server.
    pub name: String,
    pub transport: PluginMcpTransport,
}

/// The closed transport union from §7.2.1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginMcpTransport {
    Stdio {
        /// A single executable token. Never placeholder-expanded (§7.2.1).
        command: String,
        /// Placeholder-expanded per §9.2.
        args: Vec<String>,
        /// Values placeholder-expanded per §9.2; keys never are.
        env: BTreeMap<String, String>,
        /// Absolute working directory. Defaults to the plugin root when the
        /// manifest omits `cwd` (§7.2.1).
        cwd: PathBuf,
    },
    /// The current MCP Streamable HTTP transport.
    StreamableHttp {
        url: String,
        headers: BTreeMap<String, String>,
    },
    /// The deprecated HTTP+SSE transport from MCP 2024-11-05.
    Sse {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

/// Discovers MCP servers from `mcp.json` at the plugin root.
///
/// `manifest_schema` is the `$schema` value from `plugin.json`; §10.1 requires
/// `mcp.json` to declare the same specification version, and a mismatch disables
/// MCP for the plugin without touching other component types.
///
/// `plugin_data` is the client-managed persistent directory for this installed
/// plugin (§9.1), used to expand `${PLUGIN_DATA}` and to bound a
/// `${PLUGIN_DATA}`-rooted `cwd`.
pub fn discover_mcp(
    plugin_root: &Path,
    plugin_data: &Path,
    manifest_schema: &str,
) -> (McpComponent, Vec<PluginDiagnostic>) {
    let mut diagnostics = Vec::new();
    let path = plugin_root.join(AGENT_PLUGIN_MCP_RELATIVE_PATH);

    // §6.2: an absent fixed location is not an error.
    if !path.exists() {
        return (McpComponent::absent(), diagnostics);
    }

    // §6.2: present but the wrong filesystem kind invalidates this component
    // type only.
    if !path.is_file() {
        let reason = "mcp.json is not a regular file".to_string();
        diagnostics.push(PluginDiagnostic::McpDisabled {
            reason: reason.clone(),
        });
        return (McpComponent::disabled(reason), diagnostics);
    }

    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => {
            let reason = format!("cannot read mcp.json: {error}");
            diagnostics.push(PluginDiagnostic::McpDisabled {
                reason: reason.clone(),
            });
            return (McpComponent::disabled(reason), diagnostics);
        }
    };

    match parse_mcp(&contents, plugin_root, plugin_data, manifest_schema) {
        Ok((servers, mut entry_diagnostics)) => {
            diagnostics.append(&mut entry_diagnostics);
            (
                McpComponent {
                    status: McpStatus::Loaded,
                    servers,
                },
                diagnostics,
            )
        }
        Err(reason) => {
            diagnostics.push(PluginDiagnostic::McpDisabled {
                reason: reason.clone(),
            });
            (McpComponent::disabled(reason), diagnostics)
        }
    }
}

/// Parses `mcp.json` contents.
///
/// `Err` means the file as a whole is unusable (§7.2.2 rule 2). `Ok` carries the
/// valid servers plus per-entry skip diagnostics (§7.2.2 rules 3–4).
pub fn parse_mcp(
    contents: &str,
    plugin_root: &Path,
    plugin_data: &Path,
    manifest_schema: &str,
) -> Result<(Vec<PluginMcpServer>, Vec<PluginDiagnostic>), String> {
    let value: Value = serde_json::from_str(contents)
        .map_err(|error| format!("mcp.json is not valid JSON: {error}"))?;
    let Value::Object(object) = value else {
        return Err("mcp.json must be a JSON object".to_string());
    };

    // §7.2.1: the top level is closed — exactly `$schema` and `mcpServers`.
    for field in object.keys() {
        if !PERMITTED_TOP_LEVEL.contains(&field.as_str()) {
            return Err(format!("unexpected top-level field in mcp.json: {field}"));
        }
    }

    let schema = read_schema_value(contents)
        .ok_or_else(|| "mcp.json is missing the required `$schema` field".to_string())?;
    match mcp_schema_status(contents) {
        SchemaStatus::Supported => {}
        SchemaStatus::Unsupported => {
            return Err(format!(
                "unsupported Agent Plugins version in mcp.json: {schema}"
            ))
        }
        SchemaStatus::Unrelated => {
            return Err(format!(
                "`$schema` does not identify an Agent Plugins MCP config: {schema}"
            ))
        }
    }
    // §10.1: the MCP config must target the same specification version as the
    // manifest. A mismatched pair is a mixed-version package.
    if !versions_match(manifest_schema, &schema) {
        return Err(format!(
            "mcp.json targets a different Agent Plugins version than plugin.json: \
             {schema} vs {manifest_schema}"
        ));
    }

    let servers_value = object
        .get("mcpServers")
        .ok_or_else(|| "mcp.json is missing the required `mcpServers` field".to_string())?;
    let Value::Object(servers) = servers_value else {
        return Err("`mcpServers` must be an object".to_string());
    };

    let mut out = Vec::new();
    let mut diagnostics = Vec::new();
    for (name, entry) in servers {
        match parse_server(entry, plugin_root, plugin_data) {
            Ok(transport) => out.push(PluginMcpServer {
                name: name.clone(),
                transport,
            }),
            // §7.2.2 rule 3: skip this entry, keep loading the others.
            Err(reason) => diagnostics.push(PluginDiagnostic::McpServerSkipped {
                server: name.clone(),
                reason,
            }),
        }
    }

    Ok((out, diagnostics))
}

fn parse_server(
    entry: &Value,
    plugin_root: &Path,
    plugin_data: &Path,
) -> Result<PluginMcpTransport, String> {
    let Value::Object(server) = entry else {
        return Err(format!("expected an object, found {}", json_type(entry)));
    };

    let transport = server
        .get("type")
        .ok_or_else(|| "missing required `type` field".to_string())?;
    let Value::String(transport) = transport else {
        return Err(format!(
            "`type` must be a string, found {}",
            json_type(transport)
        ));
    };

    // Reject unknown fields *and* fields belonging to another variant, using the
    // permitted set for the declared type.
    let permitted = match transport.as_str() {
        "stdio" => STDIO_FIELDS,
        "streamable-http" | "sse" => REMOTE_FIELDS,
        other => return Err(format!("unknown transport type: {other}")),
    };
    for field in server.keys() {
        if !permitted.contains(&field.as_str()) {
            return Err(format!(
                "field `{field}` is not permitted on a `{transport}` server"
            ));
        }
    }

    match transport.as_str() {
        "stdio" => parse_stdio(server, plugin_root, plugin_data),
        "streamable-http" => {
            let (url, headers) = parse_remote(server)?;
            Ok(PluginMcpTransport::StreamableHttp { url, headers })
        }
        "sse" => {
            let (url, headers) = parse_remote(server)?;
            Ok(PluginMcpTransport::Sse { url, headers })
        }
        other => Err(format!("unknown transport type: {other}")),
    }
}

fn parse_stdio(
    server: &serde_json::Map<String, Value>,
    plugin_root: &Path,
    plugin_data: &Path,
) -> Result<PluginMcpTransport, String> {
    let command = string_field(server, "command")?
        .ok_or_else(|| "`command` is required for a stdio server".to_string())?;
    validate_command(&command, plugin_root)?;

    let root = plugin_root.display().to_string();
    let data = plugin_data.display().to_string();

    let args = match server.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                // §9.2: expansion applies to every string element of `args`.
                Value::String(arg) => Ok(normalize_and_expand(arg, &root, &data)),
                other => Err(format!(
                    "`args` must contain only strings, found {}",
                    json_type(other)
                )),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(other) => {
            return Err(format!(
                "`args` must be an array of strings, found {}",
                json_type(other)
            ))
        }
    };

    let env = parse_env(server, &root, &data)?;

    let cwd = match string_field(server, "cwd")? {
        Some(raw) => resolve_cwd(&raw, plugin_root, plugin_data)?,
        // §7.2.1: when `cwd` is omitted the plugin root is the working
        // directory.
        None => plugin_root.to_path_buf(),
    };

    Ok(PluginMcpTransport::Stdio {
        command,
        args,
        env,
        cwd,
    })
}

/// §7.2.1: `command` is a single executable token — a bare name or a
/// plugin-relative `./` path — and is never placeholder-expanded.
fn validate_command(command: &str, plugin_root: &Path) -> Result<(), String> {
    if command.is_empty() {
        return Err("`command` must not be empty".to_string());
    }
    if command.contains("${") {
        return Err(
            "`command` must not contain placeholders; it is a single executable token".to_string(),
        );
    }
    if command.starts_with("./") {
        resolve_plugin_relative(plugin_root, command)
            .map_err(|error| format!("`command` must stay within the plugin root: {error}"))?;
        return Ok(());
    }
    // A bare name is resolved by the platform's executable search rules. What it
    // must not be is a path or a shell string, both of which would make it more
    // than one token.
    if command.contains('/') || command.contains('\\') {
        return Err(
            "`command` must be a bare executable name or a plugin-relative `./` path".to_string(),
        );
    }
    if command.split_whitespace().count() > 1 {
        return Err("`command` must be one token, not a shell command string".to_string());
    }
    Ok(())
}

fn parse_env(
    server: &serde_json::Map<String, Value>,
    root: &str,
    data: &str,
) -> Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    match server.get("env") {
        None | Some(Value::Null) => return Ok(env),
        Some(Value::Object(entries)) => {
            for (key, value) in entries {
                let Value::String(value) = value else {
                    return Err(format!(
                        "`env` values must be strings, found {} for `{key}`",
                        json_type(value)
                    ));
                };
                // §9.2: expansion applies to values, never to keys.
                env.insert(key.clone(), normalize_and_expand(value, root, data));
            }
        }
        Some(other) => {
            return Err(format!(
                "`env` must be an object of strings, found {}",
                json_type(other)
            ))
        }
    }

    // §9.2: the client supplies PLUGIN_ROOT / PLUGIN_DATA itself, so a server
    // declaring either is invalid rather than merely overridden.
    if let Some(reserved) = reserved_env_key(&env) {
        return Err(format!(
            "`env` must not declare `{reserved}`; the client provides it"
        ));
    }
    Ok(env)
}

/// Resolves an explicit `cwd` against §7.2.1's three permitted forms, expanding
/// placeholders first and enforcing containment afterwards.
fn resolve_cwd(raw: &str, plugin_root: &Path, plugin_data: &Path) -> Result<PathBuf, String> {
    let form = classify_cwd(raw).ok_or_else(|| {
        format!(
            "`cwd` must be a plugin-relative `./` path, `${{PLUGIN_ROOT}}`, or `${{PLUGIN_DATA}}`, \
             optionally with a trailing path: {raw}"
        )
    })?;

    let (base, tail) = match form {
        CwdForm::PluginRelative(path) => {
            let resolved = resolve_plugin_relative(plugin_root, path)
                .map_err(|error| format!("invalid `cwd`: {error}"))?;
            return Ok(resolved);
        }
        CwdForm::PluginRoot { tail } => (plugin_root, tail),
        CwdForm::PluginData { tail } => (plugin_data, tail),
    };

    if tail.is_empty() {
        return Ok(base.to_path_buf());
    }
    // The tail is a path fragment, not a plugin-relative declaration, so it is
    // joined and then bounded — §7.2.1 makes any post-resolution escape invalid.
    let mut resolved = base.to_path_buf();
    for segment in tail.split(['/', '\\']) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(format!("`cwd` must not contain `..`: {tail}"));
        }
        resolved.push(segment);
    }
    if !is_within(base, &resolved) {
        return Err(format!("`cwd` resolves outside its root: {tail}"));
    }
    Ok(resolved)
}

fn parse_remote(
    server: &serde_json::Map<String, Value>,
) -> Result<(String, BTreeMap<String, String>), String> {
    let url = string_field(server, "url")?
        .ok_or_else(|| "`url` is required for a remote server".to_string())?;
    validate_remote_url(&url)?;

    let mut headers = BTreeMap::new();
    match server.get("headers") {
        None | Some(Value::Null) => {}
        Some(Value::Object(entries)) => {
            let mut seen = BTreeSet::new();
            for (name, value) in entries {
                let Value::String(value) = value else {
                    return Err(format!(
                        "`headers` values must be strings, found {} for `{name}`",
                        json_type(value)
                    ));
                };
                validate_header_name(name)?;
                validate_header_value(name, value)?;
                // §7.2.1: header names are case-insensitive, so the same name
                // under different casing is a duplicate and invalidates the
                // entry.
                if !seen.insert(name.to_ascii_lowercase()) {
                    return Err(format!(
                        "`headers` contains `{name}` more than once under different casing"
                    ));
                }
                headers.insert(name.clone(), value.clone());
            }
        }
        Some(other) => {
            return Err(format!(
                "`headers` must be an object of strings, found {}",
                json_type(other)
            ))
        }
    }

    Ok((url, headers))
}

/// §7.2.1: absolute HTTP/HTTPS, no user information, no fragment, and HTTPS
/// unless the host is a loopback address.
fn validate_remote_url(raw: &str) -> Result<(), String> {
    let lower = raw.to_ascii_lowercase();
    let (is_https, rest) = if let Some(rest) = lower.strip_prefix("https://") {
        (true, &raw[raw.len() - rest.len()..])
    } else if let Some(rest) = lower.strip_prefix("http://") {
        (false, &raw[raw.len() - rest.len()..])
    } else {
        return Err(format!(
            "`url` must be an absolute http or https URL: {raw}"
        ));
    };

    if raw.contains('#') {
        return Err(format!("`url` must not contain a fragment: {raw}"));
    }

    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err(format!("`url` is missing a host: {raw}"));
    }
    if authority.contains('@') {
        return Err(format!("`url` must not contain user information: {raw}"));
    }

    let host = host_of(authority);
    if host.is_empty() {
        return Err(format!("`url` is missing a host: {raw}"));
    }
    if !is_https && !is_loopback_host(&host) {
        return Err(format!("a non-loopback `url` must use https: {raw}"));
    }
    Ok(())
}

/// Strips the port from an authority, handling bracketed IPv6 literals.
fn host_of(authority: &str) -> String {
    if let Some(rest) = authority.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((host, _)) => host.to_string(),
            None => String::new(),
        };
    }
    match authority.split_once(':') {
        Some((host, _)) => host.to_string(),
        None => authority.to_string(),
    }
}

/// §7.2.1 permits plain HTTP when the host is exactly `localhost` or an IP
/// literal in a loopback range.
fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(addr) = host.parse::<std::net::IpAddr>() {
        return addr.is_loopback();
    }
    false
}

/// RFC 7230 token characters.
fn validate_header_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("`headers` contains an empty header name".to_string());
    }
    const EXTRA: &[u8] = b"!#$%&'*+-.^_`|~";
    for byte in name.bytes() {
        if !(byte.is_ascii_alphanumeric() || EXTRA.contains(&byte)) {
            return Err(format!("invalid character in header name `{name}`"));
        }
    }
    Ok(())
}

/// Visible ASCII plus space and horizontal tab; no CR or LF, which would allow
/// header injection.
fn validate_header_value(name: &str, value: &str) -> Result<(), String> {
    for byte in value.bytes() {
        let printable = (0x20..=0x7e).contains(&byte);
        if !(printable || byte == b'\t') {
            return Err(format!("invalid character in value of header `{name}`"));
        }
    }
    Ok(())
}

fn string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<String>, String> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Err(format!(
            "`{field}` must be a string, found {}",
            json_type(other)
        )),
    }
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
    use crate::plugin::spec::schema::{AGENT_PLUGIN_MCP_SCHEMA_URI, AGENT_PLUGIN_SCHEMA_URI};

    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\plugins\demo"
        } else {
            "/plugins/demo"
        })
    }

    fn data() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\data\demo"
        } else {
            "/data/demo"
        })
    }

    fn config(servers: &str) -> String {
        format!(r#"{{"$schema": "{AGENT_PLUGIN_MCP_SCHEMA_URI}", "mcpServers": {servers}}}"#)
    }

    fn parse(servers: &str) -> Result<(Vec<PluginMcpServer>, Vec<PluginDiagnostic>), String> {
        parse_mcp(&config(servers), &root(), &data(), AGENT_PLUGIN_SCHEMA_URI)
    }

    /// The complete `mcp.json` example from §7.2.1, all three transports.
    #[test]
    fn accepts_spec_example_with_all_three_transports() {
        let servers = r#"{
            "local-validator": {
              "type": "stdio",
              "command": "./bin/validator",
              "args": ["--data", "${PLUGIN_DATA}/validator"],
              "env": { "CONFIG": "${PLUGIN_ROOT}/config.json" },
              "cwd": "${PLUGIN_ROOT}"
            },
            "deployment-api": {
              "type": "streamable-http",
              "url": "https://deploy.example.com/mcp",
              "headers": { "X-Tenant": "public-tenant" }
            },
            "legacy-events": {
              "type": "sse",
              "url": "https://legacy.example.com/sse"
            }
          }"#;

        let (parsed, diagnostics) = parse(servers).expect("file is valid");

        assert!(diagnostics.is_empty(), "unexpected: {diagnostics:?}");
        assert_eq!(parsed.len(), 3);

        let validator = parsed
            .iter()
            .find(|s| s.name == "local-validator")
            .expect("validator");
        match &validator.transport {
            PluginMcpTransport::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                assert_eq!(command, "./bin/validator");
                assert_eq!(args[0], "--data");
                assert!(args[1].ends_with("validator"));
                assert!(args[1].starts_with(&data().display().to_string()));
                assert!(env["CONFIG"].starts_with(&root().display().to_string()));
                assert_eq!(cwd, &root());
            }
            other => panic!("expected stdio, got {other:?}"),
        }

        assert!(matches!(
            parsed
                .iter()
                .find(|s| s.name == "deployment-api")
                .map(|s| &s.transport),
            Some(PluginMcpTransport::StreamableHttp { .. })
        ));
        assert!(matches!(
            parsed
                .iter()
                .find(|s| s.name == "legacy-events")
                .map(|s| &s.transport),
            Some(PluginMcpTransport::Sse { .. })
        ));
    }

    /// §7.2.2 rule 3: one bad entry must not take down its siblings.
    #[test]
    fn one_invalid_server_does_not_affect_the_others() {
        let servers = r#"{
            "good": { "type": "stdio", "command": "node" },
            "bad": { "type": "carrier-pigeon" },
            "also-good": { "type": "sse", "url": "https://example.com/sse" }
          }"#;

        let (parsed, diagnostics) = parse(servers).expect("file stays usable");

        assert_eq!(parsed.len(), 2);
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { server, reason }
                if server == "bad" && reason.contains("unknown transport type")
        ));
    }

    #[test]
    fn empty_mcp_servers_object_is_valid() {
        let (parsed, diagnostics) = parse("{}").expect("valid");
        assert!(parsed.is_empty());
        assert!(diagnostics.is_empty());
    }

    /// §7.2.1: the top level is closed, and a stray field disables MCP for the
    /// whole plugin rather than one entry.
    #[test]
    fn extra_top_level_field_disables_the_file() {
        let contents = format!(
            r#"{{"$schema": "{AGENT_PLUGIN_MCP_SCHEMA_URI}", "mcpServers": {{}}, "extra": 1}}"#
        );
        let err = parse_mcp(&contents, &root(), &data(), AGENT_PLUGIN_SCHEMA_URI)
            .expect_err("whole file is unusable");
        assert!(err.contains("unexpected top-level field"));
    }

    /// §10.1: a version mismatch between the two schemas is a mixed-version
    /// package.
    #[test]
    fn schema_version_mismatch_disables_the_file() {
        let contents =
            format!(r#"{{"$schema": "{AGENT_PLUGIN_MCP_SCHEMA_URI}", "mcpServers": {{}}}}"#);
        let err = parse_mcp(
            &contents,
            &root(),
            &data(),
            "https://agent-plugins.org/schemas/2.0.0/plugin.schema.json",
        )
        .expect_err("mismatch");
        assert!(err.contains("different Agent Plugins version"));
    }

    #[test]
    fn missing_or_foreign_schema_disables_the_file() {
        let err = parse_mcp(
            r#"{"mcpServers": {}}"#,
            &root(),
            &data(),
            AGENT_PLUGIN_SCHEMA_URI,
        )
        .expect_err("missing schema");
        assert!(err.contains("$schema"));

        let err = parse_mcp(
            r#"{"$schema": "https://example.com/mcp.json", "mcpServers": {}}"#,
            &root(),
            &data(),
            AGENT_PLUGIN_SCHEMA_URI,
        )
        .expect_err("foreign schema");
        assert!(err.contains("does not identify"));
    }

    #[test]
    fn missing_mcp_servers_disables_the_file() {
        let contents = format!(r#"{{"$schema": "{AGENT_PLUGIN_MCP_SCHEMA_URI}"}}"#);
        let err =
            parse_mcp(&contents, &root(), &data(), AGENT_PLUGIN_SCHEMA_URI).expect_err("missing");
        assert!(err.contains("mcpServers"));
    }

    /// §7.2.1: a field belonging to another variant invalidates the entry —
    /// this is what makes the union closed rather than merely tagged.
    #[test]
    fn cross_variant_fields_are_rejected() {
        let (_, diagnostics) =
            parse(r#"{"mixed": {"type": "stdio", "command": "node", "url": "https://x.test"}}"#)
                .expect("file usable");
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { reason, .. }
                if reason.contains("`url` is not permitted on a `stdio` server")
        ));

        let (_, diagnostics) =
            parse(r#"{"mixed": {"type": "sse", "url": "https://x.test", "command": "node"}}"#)
                .expect("file usable");
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { reason, .. }
                if reason.contains("`command` is not permitted on a `sse` server")
        ));
    }

    #[test]
    fn unknown_field_on_a_server_is_rejected() {
        let (_, diagnostics) =
            parse(r#"{"s": {"type": "stdio", "command": "node", "timeout": 5}}"#)
                .expect("file usable");
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { reason, .. } if reason.contains("timeout")
        ));
    }

    #[test]
    fn missing_type_is_rejected() {
        let (_, diagnostics) = parse(r#"{"s": {"command": "node"}}"#).expect("file usable");
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { reason, .. } if reason.contains("`type`")
        ));
    }

    /// §7.2.1: `command` is one token and is never placeholder-expanded.
    #[test]
    fn command_must_be_a_single_token() {
        for (command, expected) in [
            ("", "must not be empty"),
            ("${PLUGIN_ROOT}/bin/x", "must not contain placeholders"),
            ("bin/server", "bare executable name"),
            ("/usr/bin/node", "bare executable name"),
            ("node --inspect", "one token"),
        ] {
            let servers = format!(r#"{{"s": {{"type": "stdio", "command": "{command}"}}}}"#);
            let (_, diagnostics) = parse(&servers).expect("file usable");
            assert!(
                matches!(
                    &diagnostics[0],
                    PluginDiagnostic::McpServerSkipped { reason, .. } if reason.contains(expected)
                ),
                "command {command:?} should be rejected with {expected:?}, got {diagnostics:?}"
            );
        }
    }

    #[test]
    fn command_accepts_bare_name_and_plugin_relative_path() {
        let (parsed, diagnostics) =
            parse(r#"{"a": {"type": "stdio", "command": "npx"}}"#).expect("valid");
        assert!(diagnostics.is_empty());
        assert_eq!(parsed.len(), 1);

        let (parsed, diagnostics) =
            parse(r#"{"a": {"type": "stdio", "command": "./bin/server"}}"#).expect("valid");
        assert!(diagnostics.is_empty());
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn plugin_relative_command_escaping_the_root_is_rejected() {
        let (_, diagnostics) =
            parse(r#"{"s": {"type": "stdio", "command": "./../evil"}}"#).expect("file usable");
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { reason, .. }
                if reason.contains("within the plugin root")
        ));
    }

    /// §9.2: the client supplies these itself, so declaring them is invalid.
    #[test]
    fn reserved_env_keys_invalidate_the_server() {
        for reserved in ["PLUGIN_ROOT", "PLUGIN_DATA"] {
            let servers = format!(
                r#"{{"s": {{"type": "stdio", "command": "node", "env": {{"{reserved}": "/x"}}}}}}"#
            );
            let (_, diagnostics) = parse(&servers).expect("file usable");
            assert!(
                matches!(
                    &diagnostics[0],
                    PluginDiagnostic::McpServerSkipped { reason, .. }
                        if reason.contains(reserved) && reason.contains("client provides it")
                ),
                "{reserved} should invalidate the server, got {diagnostics:?}"
            );
        }
    }

    /// §9.2: expansion applies to env *values*, never to keys.
    #[test]
    fn env_keys_are_not_expanded() {
        let servers = r#"{"s": {"type": "stdio", "command": "node",
                              "env": {"${PLUGIN_ROOT}": "literal-key"}}}"#;
        let (parsed, diagnostics) = parse(servers).expect("valid");
        assert!(diagnostics.is_empty());
        match &parsed[0].transport {
            PluginMcpTransport::Stdio { env, .. } => {
                assert!(env.contains_key("${PLUGIN_ROOT}"), "key must stay literal");
            }
            other => panic!("expected stdio, got {other:?}"),
        }
    }

    /// §9.2: unrecognized placeholders stay literal rather than becoming empty.
    #[test]
    fn unknown_placeholders_in_args_stay_literal() {
        let servers = r#"{"s": {"type": "stdio", "command": "node",
                              "args": ["${API_TOKEN}"]}}"#;
        let (parsed, _) = parse(servers).expect("valid");
        match &parsed[0].transport {
            PluginMcpTransport::Stdio { args, .. } => assert_eq!(args[0], "${API_TOKEN}"),
            other => panic!("expected stdio, got {other:?}"),
        }
    }

    /// §7.2.1: `cwd` defaults to the plugin root.
    #[test]
    fn omitted_cwd_defaults_to_the_plugin_root() {
        let (parsed, _) = parse(r#"{"s": {"type": "stdio", "command": "node"}}"#).expect("valid");
        match &parsed[0].transport {
            PluginMcpTransport::Stdio { cwd, .. } => assert_eq!(cwd, &root()),
            other => panic!("expected stdio, got {other:?}"),
        }
    }

    #[test]
    fn accepts_the_three_permitted_cwd_forms() {
        let cases = [
            ("./data", root().join("data")),
            ("${PLUGIN_ROOT}", root()),
            ("${PLUGIN_ROOT}/sub", root().join("sub")),
            ("${PLUGIN_DATA}", data()),
            ("${PLUGIN_DATA}/cache", data().join("cache")),
        ];
        for (raw, expected) in cases {
            let servers =
                format!(r#"{{"s": {{"type": "stdio", "command": "node", "cwd": "{raw}"}}}}"#);
            let (parsed, diagnostics) = parse(&servers).expect("file usable");
            assert!(diagnostics.is_empty(), "{raw}: {diagnostics:?}");
            match &parsed[0].transport {
                PluginMcpTransport::Stdio { cwd, .. } => assert_eq!(cwd, &expected, "{raw}"),
                other => panic!("expected stdio, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_other_cwd_shapes_and_escapes() {
        for (raw, expected) in [
            ("data", "must be a plugin-relative"),
            ("/absolute", "must be a plugin-relative"),
            ("../escape", "must be a plugin-relative"),
            ("${PLUGIN_ROOT}/../escape", "must not contain"),
            ("${PLUGIN_DATA}/../escape", "must not contain"),
            ("./../escape", "invalid `cwd`"),
        ] {
            let servers =
                format!(r#"{{"s": {{"type": "stdio", "command": "node", "cwd": "{raw}"}}}}"#);
            let (_, diagnostics) = parse(&servers).expect("file usable");
            assert!(
                matches!(
                    &diagnostics[0],
                    PluginDiagnostic::McpServerSkipped { reason, .. } if reason.contains(expected)
                ),
                "cwd {raw:?} should be rejected with {expected:?}, got {diagnostics:?}"
            );
        }
    }

    /// §7.2.1: non-loopback endpoints must use HTTPS; loopback may use HTTP.
    #[test]
    fn remote_url_scheme_rules() {
        let accepted = [
            "https://example.com/mcp",
            "http://localhost:3000/mcp",
            "http://127.0.0.1/mcp",
            "http://[::1]:8080/mcp",
            "https://LOCALHOST/mcp",
        ];
        for url in accepted {
            let servers = format!(r#"{{"s": {{"type": "sse", "url": "{url}"}}}}"#);
            let (parsed, diagnostics) = parse(&servers).expect("file usable");
            assert!(
                diagnostics.is_empty(),
                "{url} should be accepted: {diagnostics:?}"
            );
            assert_eq!(parsed.len(), 1);
        }

        let rejected = [
            ("http://example.com/mcp", "must use https"),
            ("ftp://example.com/mcp", "absolute http or https"),
            ("example.com/mcp", "absolute http or https"),
            ("https://user:pass@example.com/mcp", "user information"),
            ("https://example.com/mcp#frag", "fragment"),
            ("https:///mcp", "missing a host"),
        ];
        for (url, expected) in rejected {
            let servers = format!(r#"{{"s": {{"type": "streamable-http", "url": "{url}"}}}}"#);
            let (_, diagnostics) = parse(&servers).expect("file usable");
            assert!(
                matches!(
                    &diagnostics[0],
                    PluginDiagnostic::McpServerSkipped { reason, .. } if reason.contains(expected)
                ),
                "url {url:?} should be rejected with {expected:?}, got {diagnostics:?}"
            );
        }
    }

    #[test]
    fn remote_url_is_required() {
        let (_, diagnostics) = parse(r#"{"s": {"type": "sse"}}"#).expect("file usable");
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { reason, .. } if reason.contains("`url` is required")
        ));
    }

    /// §7.2.1: header names are case-insensitive, so differing casing of the
    /// same name is a duplicate.
    #[test]
    fn duplicate_header_names_under_different_casing_are_rejected() {
        let servers = r#"{"s": {"type": "sse", "url": "https://x.test/sse",
                              "headers": {"X-Tenant": "a", "x-tenant": "b"}}}"#;
        let (_, diagnostics) = parse(servers).expect("file usable");
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { reason, .. }
                if reason.contains("more than once under different casing")
        ));
    }

    /// A CR or LF in a header value would allow header injection.
    #[test]
    fn rejects_invalid_header_names_and_values() {
        let servers = r#"{"s": {"type": "sse", "url": "https://x.test/sse",
                              "headers": {"Bad Name": "v"}}}"#;
        let (_, diagnostics) = parse(servers).expect("file usable");
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { reason, .. }
                if reason.contains("invalid character in header name")
        ));

        let servers = r#"{"s": {"type": "sse", "url": "https://x.test/sse",
                              "headers": {"X-A": "line1\nline2"}}}"#;
        let (_, diagnostics) = parse(servers).expect("file usable");
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpServerSkipped { reason, .. }
                if reason.contains("invalid character in value")
        ));
    }

    /// §9.2 forbids expanding `url` and header values, so a placeholder there
    /// must survive as literal text.
    #[test]
    fn remote_fields_are_not_placeholder_expanded() {
        let servers = r#"{"s": {"type": "sse", "url": "https://x.test/sse",
                              "headers": {"X-Root": "${PLUGIN_ROOT}"}}}"#;
        let (parsed, diagnostics) = parse(servers).expect("valid");
        assert!(diagnostics.is_empty());
        match &parsed[0].transport {
            PluginMcpTransport::Sse { headers, .. } => {
                assert_eq!(headers["X-Root"], "${PLUGIN_ROOT}");
            }
            other => panic!("expected sse, got {other:?}"),
        }
    }

    #[test]
    fn rejects_wrong_field_types() {
        for (servers, expected) in [
            (r#"{"s": {"type": 1}}"#, "`type` must be a string"),
            (r#"{"s": "not an object"}"#, "expected an object"),
            (
                r#"{"s": {"type": "stdio", "command": "node", "args": "x"}}"#,
                "`args` must be an array",
            ),
            (
                r#"{"s": {"type": "stdio", "command": "node", "args": [1]}}"#,
                "only strings",
            ),
            (
                r#"{"s": {"type": "stdio", "command": "node", "env": []}}"#,
                "`env` must be an object",
            ),
            (
                r#"{"s": {"type": "sse", "url": "https://x.test", "headers": 1}}"#,
                "`headers` must be an object",
            ),
        ] {
            let (_, diagnostics) = parse(servers).expect("file usable");
            assert!(
                matches!(
                    &diagnostics[0],
                    PluginDiagnostic::McpServerSkipped { reason, .. } if reason.contains(expected)
                ),
                "{servers} should be rejected with {expected:?}, got {diagnostics:?}"
            );
        }
    }

    #[test]
    fn malformed_or_non_object_file_is_disabled() {
        assert!(
            parse_mcp("{not json", &root(), &data(), AGENT_PLUGIN_SCHEMA_URI)
                .expect_err("malformed")
                .contains("not valid JSON")
        );
        assert!(parse_mcp("[]", &root(), &data(), AGENT_PLUGIN_SCHEMA_URI)
            .expect_err("array")
            .contains("must be a JSON object"));
    }

    #[test]
    fn non_object_mcp_servers_disables_the_file() {
        let err = parse("[]").expect_err("array");
        assert!(err.contains("`mcpServers` must be an object"));
    }

    // ---- filesystem-level discovery -------------------------------------

    #[test]
    fn missing_mcp_json_is_absent_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (component, diagnostics) =
            discover_mcp(tmp.path(), tmp.path(), AGENT_PLUGIN_SCHEMA_URI);
        assert_eq!(component.status, McpStatus::Absent);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn mcp_json_as_a_directory_disables_the_component() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("mcp.json")).expect("create dir");

        let (component, diagnostics) =
            discover_mcp(tmp.path(), tmp.path(), AGENT_PLUGIN_SCHEMA_URI);

        assert!(matches!(component.status, McpStatus::Disabled { .. }));
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            &diagnostics[0],
            PluginDiagnostic::McpDisabled { reason } if reason.contains("not a regular file")
        ));
    }

    #[test]
    fn discovers_servers_from_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let plugin_root = tmp.path().join("plugin");
        let plugin_data = tmp.path().join("data");
        std::fs::create_dir_all(&plugin_root).expect("create root");
        std::fs::write(
            plugin_root.join("mcp.json"),
            config(r#"{"api": {"type": "streamable-http", "url": "https://x.test/mcp"}}"#),
        )
        .expect("write");

        let (component, diagnostics) =
            discover_mcp(&plugin_root, &plugin_data, AGENT_PLUGIN_SCHEMA_URI);

        assert_eq!(component.status, McpStatus::Loaded);
        assert_eq!(component.servers.len(), 1);
        assert_eq!(component.servers[0].name, "api");
        assert!(diagnostics.is_empty());
    }
}
