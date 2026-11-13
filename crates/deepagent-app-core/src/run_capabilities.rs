//! Capability evidence projected from the one real main-run registry.
//!
//! Only hashes and routing metadata are durable: tool descriptions, schema
//! bodies, prompts, raw settings and credentials must not become another log.
//! This is not an authorization engine; the existing tool pipeline owns that.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_models::ToolSchema;
use deepagent_persistence::{
    run_store::{CapabilitySchema, RunStore},
    Database,
};
use deepagent_tools::{PermissionSet, ToolRegistry};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::mcp_runtime::McpLifecycleRecord;
use crate::sandbox_backend::{
    DirectSandboxBackend, SandboxBackend, SandboxBackendKind, SandboxBackendRegistry,
};
use crate::settings::{ApprovalPolicy, EffectivePermissionProfile};

pub(crate) struct CapabilityContextRequest<'a> {
    pub(crate) root: &'a Path,
    pub(crate) profile: EffectivePermissionProfile,
    pub(crate) permission_rules: &'a deepagent_hooks::PermissionRules,
    pub(crate) sandbox_backend: Option<SandboxBackendKind>,
    pub(crate) backends: Option<&'a SandboxBackendRegistry>,
    pub(crate) tool_search_mode: deepagent_builtins::ToolSearchMode,
    pub(crate) tool_search_threshold: usize,
    pub(crate) mcp_lifecycle: &'a [McpLifecycleRecord],
}

/// Read only metadata from the effective environment already chosen by the
/// assembler. Never resolve a second permission policy or select a fallback.
pub(crate) fn capability_context(request: CapabilityContextRequest<'_>) -> Result<Value> {
    let capabilities = request
        .sandbox_backend
        .and_then(|kind| {
            request
                .backends
                .and_then(|registry| registry.capabilities(kind))
                .or_else(|| {
                    (kind == SandboxBackendKind::Direct)
                        .then(|| DirectSandboxBackend::new().capabilities())
                })
        })
        .map(|capabilities| {
            json!({
                "kind": capabilities.kind,
                "available": capabilities.available,
                "supportsOneShot": capabilities.supports_one_shot,
                "supportsInteractivePty": capabilities.supports_interactive_pty,
                "supportsNetworkToggle": capabilities.supports_network_toggle,
                "supportsReadonlyMapping": capabilities.supports_readonly_mapping,
            })
        });
    let profile = request.profile;
    let permission_profile = json!({
        "approvalPolicy": profile.approval_policy.label(),
        "sandboxMode": profile.sandbox_mode.label(),
        "localExecutionMode": format!("{:?}", profile.local_execution_mode),
        "networkAlwaysAsk": profile.network_always_ask,
    });
    let mcp_schemas = request
        .mcp_lifecycle
        .iter()
        .map(|record| {
            json!({
                "serverId": record.server_id,
                "status": record.status,
                "configHash": record.config_hash,
                "toolSchemaHash": record.tool_schema_hash,
                "toolCount": record.tool_count,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "workspace": std::fs::canonicalize(request.root).map_err(|error|
            CoreError::other(format!("canonicalize capability workspace: {error}")))?,
        "permissionProfile": permission_profile,
        "permissionProfileHash": capability_hash(&permission_profile)?,
        "permissionRulesHash": capability_hash(request.permission_rules)?,
        "sandboxMode": profile.sandbox_mode.label(),
        "sandboxBackend": request.sandbox_backend.map(SandboxBackendKind::label),
        "sandboxCapabilities": capabilities,
        "toolSearchMode": request.tool_search_mode.label(),
        "toolSearchThreshold": request.tool_search_threshold,
        "mcpSchemas": mcp_schemas,
    }))
}

/// Content-address JSON independently of map insertion order. Array order is
/// retained, including the actual model tool ordering.
pub(crate) fn capability_hash(value: &(impl Serialize + ?Sized)) -> Result<String> {
    fn canonical(value: Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
            other => other,
        }
    }
    let value = canonical(serde_json::to_value(value)?);
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}

struct SchemaGeneration {
    hash: String,
    number: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaReference {
    name: String,
    schema_hash: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaProjection {
    generation: u64,
    schema_hash: String,
    tools: Vec<SchemaReference>,
}

/// A run-local owner over the existing RunStore. It never dispatches tools or
/// refreshes a second registry; each input is the exact schema vector consumed
/// by ModelAgent's normal source seam.
pub(crate) struct RunCapabilityRecorder {
    db: Arc<Database>,
    run_id: String,
    snapshot_id: String,
    catalog: BTreeMap<String, String>,
    state: Mutex<SchemaGeneration>,
    wire_state: Mutex<Option<SchemaGeneration>>,
}

/// A pure projection, separate from the later durable run binding. Admission
/// can carry this exact value alongside the prepared registry, not recompute
/// an approximation of the capabilities advertised to a client.
pub(crate) struct ResolvedRunCapabilities {
    projection: Value,
    capability_hash: String,
    catalog: BTreeMap<String, String>,
    initial_schema_hash: String,
}

/// The exact immutable value carried by the existing admission transaction.
/// Keeping its recorder state here prevents a second registry resolution.
pub(crate) struct PreparedCapabilitySnapshot {
    pub(crate) snapshot: Value,
    catalog: BTreeMap<String, String>,
    initial_schema_hash: String,
}

impl PreparedCapabilitySnapshot {
    pub(crate) fn consume(self, db: Arc<Database>) -> Result<RunCapabilityRecorder> {
        let run_id = self.snapshot["runId"]
            .as_str()
            .ok_or_else(|| CoreError::invalid("prepared capability run identity is missing"))?;
        let stored = RunStore::new(&db)
            .capability_snapshot(run_id)?
            .ok_or_else(|| CoreError::invalid("prepared capability admission is missing"))?;
        if stored != self.snapshot {
            return Err(CoreError::invalid(
                "prepared capability admission does not match",
            ));
        }
        let run_id = run_id.to_owned();
        let snapshot_id = self.snapshot["snapshotId"]
            .as_str()
            .ok_or_else(|| CoreError::invalid("prepared capability snapshot identity is missing"))?
            .to_owned();
        Ok(RunCapabilityRecorder {
            db,
            run_id,
            snapshot_id,
            catalog: self.catalog,
            state: Mutex::new(SchemaGeneration {
                hash: self.initial_schema_hash,
                number: 0,
            }),
            wire_state: Mutex::new(None),
        })
    }
}

impl ResolvedRunCapabilities {
    pub(crate) fn resolve(
        context: Value,
        registry: &ToolRegistry,
        tools: &[ToolSchema],
        policy: ApprovalPolicy,
    ) -> Result<Self> {
        let visible: BTreeSet<_> = tools.iter().map(|t| t.function.name.as_str()).collect();
        let mut catalog = BTreeMap::new();
        let projected = registry
            .iter_specs()
            .map(|spec| {
                let descriptor = &spec.descriptor;
                let schema_hash = capability_hash(&ToolSchema::function(
                    &descriptor.name,
                    &descriptor.description,
                    descriptor.parameters.clone(),
                ))?;
                catalog.insert(descriptor.name.clone(), schema_hash.clone());
                Ok(json!({
                    "name": descriptor.name,
                    "schemaHash": schema_hash,
                    "risk": descriptor.risk,
                    "requiredPermissions": descriptor.required_permissions,
                    // `None` means host-owned, not necessarily a built-in.
                    "source": spec.tool.discovery_source().unwrap_or("host"),
                    "eligibleForDeferral": spec.tool.should_defer() && !spec.tool.always_load(),
                    "initiallyVisible": visible.contains(descriptor.name.as_str()),
                    "grantedByRegistry": PermissionSet::developer().grants_all(&descriptor.required_permissions),
                    "approval": {
                        "evaluation": "invocation",
                        "riskGate": descriptor.risk.requires_approval(),
                        "policy": policy.label(),
                    },
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        let schema = schema_projection(&catalog, tools, 0)?;
        let fingerprint = deepagent_runtime::kernel::registry_capability_fingerprint(registry)?;
        let projection = json!({
            "version": 1,
            "context": context,
            "tools": projected,
            "modelSchema": schema,
            "toolCount": fingerprint.tool_count,
            "toolSchemaHash": fingerprint.tool_schema_hash,
            "authorization": "registry_visibility_is_not_execution_authorization",
        });
        Ok(Self {
            capability_hash: capability_hash(&projection)?,
            projection,
            catalog,
            initial_schema_hash: schema.schema_hash,
        })
    }

    pub(crate) fn prepare(
        self,
        run_id: &str,
        session_id: &str,
    ) -> Result<PreparedCapabilitySnapshot> {
        let mut snapshot = self.projection;
        snapshot["runId"] = json!(run_id);
        snapshot["threadId"] = json!(session_id);
        snapshot["capabilityHash"] = json!(self.capability_hash);
        snapshot["snapshotId"] = json!(capability_hash(&snapshot)?);
        Ok(PreparedCapabilitySnapshot {
            snapshot,
            catalog: self.catalog,
            initial_schema_hash: self.initial_schema_hash,
        })
    }

    #[cfg(test)]
    pub(crate) fn bind(self, db: Arc<Database>, run_id: &str) -> Result<RunCapabilityRecorder> {
        let run = RunStore::new(&db)
            .get(run_id)?
            .ok_or_else(|| CoreError::invalid("capability snapshot has no admitted run"))?;
        if run.finished_at.is_some() {
            return Err(CoreError::invalid(
                "cannot bind capabilities to a terminal run",
            ));
        }
        let prepared = self.prepare(run_id, &run.session_id)?;
        RunStore::new(&db).bind_capability_snapshot(
            run_id,
            &prepared.snapshot,
            SystemClock.now().as_millis(),
        )?;
        prepared.consume(db)
    }
}

impl RunCapabilityRecorder {
    fn record_wire_tools(
        &self,
        source: &[ToolSchema],
        wire: &deepagent_models::ProviderToolSchemas,
    ) -> Result<()> {
        // Host membership and provider serialization are distinct hash domains.
        // A native tool can have no wire name; the adapter retains source order.
        let source = schema_projection(&self.catalog, source, 0)?;
        if source.tools.len() != wire.tools.len() {
            return Err(CoreError::invalid(
                "wire schema does not match source tool count",
            ));
        }
        let tools = source
            .tools
            .iter()
            .zip(&wire.tools)
            .map(|(source, schema)| {
                Ok(json!({
                    "name": source.name,
                    "kind": schema.get("type").and_then(Value::as_str)
                        .ok_or_else(|| CoreError::invalid("wire schema has no kind"))?,
                    "schemaHash": capability_hash(schema)?,
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut projection = json!({
            "wireMode": match wire.wire_mode {
                deepagent_models::WireMode::Responses => "responses",
                deepagent_models::WireMode::ChatCompletions => "chat_completions",
            },
            "model": wire.model,
            "sourceSchemaHash": source.schema_hash,
            "schemaHash": capability_hash(&wire.tools)?,
            "tools": tools,
        });
        let hash = capability_hash(&projection)?;
        let mut state = self
            .wire_state
            .lock()
            .map_err(|_| CoreError::other("wire capability generation mutex poisoned"))?;
        let number = match state.as_ref() {
            None => 0,
            Some(state) if state.hash == hash => state.number,
            Some(state) => state
                .number
                .checked_add(1)
                .ok_or_else(|| CoreError::invalid("wire capability generation exhausted"))?,
        };
        projection["generation"] = json!(number);
        RunStore::new(&self.db).record_capability_schema(
            &self.run_id,
            &self.snapshot_id,
            CapabilitySchema::Wire(&projection),
            SystemClock.now().as_millis(),
        )?;
        *state = Some(SchemaGeneration { hash, number });
        Ok(())
    }

    pub(crate) fn wire_observer(
        self: Arc<Self>,
    ) -> deepagent_runtime::model_agent::ToolSchemaObserver {
        Arc::new(move |source, wire| self.record_wire_tools(source, wire))
    }

    pub(crate) fn record_model_tools(&self, tools: &[ToolSchema]) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoreError::other("capability generation mutex poisoned"))?;
        let mut schema = schema_projection(&self.catalog, tools, state.number)?;
        if schema.schema_hash != state.hash {
            schema.generation = state
                .number
                .checked_add(1)
                .ok_or_else(|| CoreError::invalid("capability schema generation exhausted"))?;
        }
        RunStore::new(&self.db).record_capability_schema(
            &self.run_id,
            &self.snapshot_id,
            CapabilitySchema::Model(&serde_json::to_value(&schema)?),
            SystemClock.now().as_millis(),
        )?;
        state.hash = schema.schema_hash;
        state.number = schema.generation;
        Ok(())
    }

    pub(crate) fn schema_source(
        self: Arc<Self>,
        source: Arc<dyn Fn() -> Vec<ToolSchema> + Send + Sync>,
    ) -> deepagent_runtime::model_agent::ToolSchemaSource {
        Arc::new(move || {
            let tools = source();
            self.record_model_tools(&tools)?;
            Ok(tools)
        })
    }
}

fn schema_projection(
    catalog: &BTreeMap<String, String>,
    tools: &[ToolSchema],
    generation: u64,
) -> Result<SchemaProjection> {
    let mut seen = BTreeSet::new();
    let schemas = tools
        .iter()
        .map(|tool| {
            let name = &tool.function.name;
            let hash = capability_hash(tool)?;
            if !seen.insert(name) || catalog.get(name) != Some(&hash) {
                return Err(CoreError::invalid(
                    "model schema is not part of the resolved run capability snapshot",
                ));
            }
            Ok(SchemaReference {
                name: name.clone(),
                schema_hash: hash,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(SchemaProjection {
        generation,
        schema_hash: capability_hash(tools)?,
        tools: schemas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_tools::{RiskLevel, Tool, ToolDescriptor, ToolOutput};

    struct CatalogTool(&'static str);

    #[async_trait::async_trait]
    impl Tool for CatalogTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.0.into(),
                description: "PRIVATE_SCHEMA_DESCRIPTION_NOT_FOR_LOGS".into(),
                parameters: json!({"type":"object"}),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }

        async fn invoke(&self, _: Value) -> Result<ToolOutput> {
            panic!("projection must not invoke a tool")
        }
    }

    fn fixture() -> (Arc<Database>, ToolRegistry, Vec<ToolSchema>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = deepagent_session::Session::create(&db, &SystemClock, None).unwrap();
        RunStore::new(&db)
            .create("h219-unit", &session.id().to_string(), None, 1)
            .unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(CatalogTool("first"))).unwrap();
        registry.register(Arc::new(CatalogTool("second"))).unwrap();
        let tools = registry
            .visible_to(&PermissionSet::developer())
            .into_iter()
            .map(|d| ToolSchema::function(d.name, d.description, d.parameters))
            .collect();
        (db, registry, tools)
    }

    fn recorder(
        db: &Arc<Database>,
        registry: &ToolRegistry,
        tools: &[ToolSchema],
    ) -> RunCapabilityRecorder {
        ResolvedRunCapabilities::resolve(json!({}), registry, tools, ApprovalPolicy::AlwaysAsk)
            .unwrap()
            .bind(db.clone(), "h219-unit")
            .unwrap()
    }

    #[test]
    fn h219_registered_custom_test_tool_keeps_host_and_wire_hash_domains_distinct() {
        // This is an explicit test registry, not a claim that the production
        // main registry contains an apply_patch tool.
        let (db, mut registry, _) = fixture();
        registry
            .register(Arc::new(CatalogTool("apply_patch")))
            .unwrap();
        let descriptor = registry.get("apply_patch").unwrap().descriptor.clone();
        let tools = vec![ToolSchema::function(
            descriptor.name,
            descriptor.description,
            descriptor.parameters,
        )];
        let recorder = recorder(&db, &registry, &tools);
        let client = deepagent_models::ModelClient::new(
            Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
            deepagent_models::ModelConfig::deepseek("synthetic-placeholder"),
        );
        let wire = client.project_tool_schemas("model", &tools);
        recorder.record_wire_tools(&tools, &wire).unwrap();
        let events = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        let saved = &events.last().unwrap().data["wireSchema"]["tools"][0];
        assert_eq!(saved["kind"], "function");
        assert_eq!(
            saved["schemaHash"],
            capability_hash(&wire.tools[0]).unwrap()
        );
        assert_ne!(saved["schemaHash"], capability_hash(&tools[0]).unwrap());
        assert!(!serde_json::to_string(&events)
            .unwrap()
            .contains("PRIVATE_SCHEMA_DESCRIPTION_NOT_FOR_LOGS"));
    }

    #[test]
    fn h219_wire_generations_are_route_aware_hashed_only_and_retry_after_write_failure() {
        let (db, registry, tools) = fixture();
        let recorder = recorder(&db, &registry, &tools);
        let client = deepagent_models::ModelClient::new(
            Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
            deepagent_models::ModelConfig::deepseek("PRIVATE_KEY_NOT_FOR_LOGS"),
        );
        let first = client.project_tool_schemas("primary", &tools);
        db.with_conn(|conn| conn.execute_batch("CREATE TRIGGER reject_wire BEFORE INSERT ON run_events WHEN NEW.event_type='capability_wire_snapshot' BEGIN SELECT RAISE(ABORT, 'synthetic wire write failure'); END;").map_err(|e| CoreError::Persistence(e.to_string()))).unwrap();
        assert!(recorder.record_wire_tools(&tools, &first).is_err());
        assert!(recorder.wire_state.lock().unwrap().is_none());
        db.with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_wire;")
                .map_err(|e| CoreError::Persistence(e.to_string()))
        })
        .unwrap();
        recorder.record_wire_tools(&tools, &first).unwrap();
        recorder.record_wire_tools(&tools, &first).unwrap();
        let fallback = client.project_tool_schemas("fallback", &tools);
        recorder.record_wire_tools(&tools, &fallback).unwrap();
        let chat = deepagent_models::ModelClient::new(
            Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
            deepagent_models::ModelConfig::deepseek_chat("PRIVATE_KEY_NOT_FOR_LOGS"),
        );
        let chat = chat.project_tool_schemas("fallback", &tools);
        recorder.record_wire_tools(&tools, &chat).unwrap();
        let bad = vec![ToolSchema::function(
            "first",
            "altered",
            json!({"type":"object"}),
        )];
        assert!(recorder
            .record_wire_tools(&bad, &client.project_tool_schemas("primary", &bad))
            .is_err());
        let events = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        let wire: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "capability_wire_snapshot")
            .collect();
        assert_eq!(wire.len(), 3);
        for (index, event) in wire.iter().enumerate() {
            assert_eq!(event.data["wireSchema"]["generation"], index);
            assert_eq!(event.data["snapshotId"], events[1].data["snapshotId"]);
        }
        assert_eq!(
            wire[0].data["wireSchema"]["schemaHash"],
            wire[1].data["wireSchema"]["schemaHash"]
        );
        assert_ne!(
            wire[1].data["wireSchema"]["schemaHash"],
            wire[2].data["wireSchema"]["schemaHash"]
        );
        let text = serde_json::to_string(&events).unwrap();
        assert!(!text.contains("PRIVATE_SCHEMA_DESCRIPTION_NOT_FOR_LOGS"));
        assert!(!text.contains("PRIVATE_KEY_NOT_FOR_LOGS"));
    }

    #[test]
    fn h219_hash_is_key_order_independent_but_schema_and_array_changes_are_distinct() {
        let left: Value = serde_json::from_str(r#"{"a":{"y":2,"x":1},"b":[1,2]}"#).unwrap();
        let right: Value = serde_json::from_str(r#"{"b":[1,2],"a":{"x":1,"y":2}}"#).unwrap();
        assert_eq!(
            capability_hash(&left).unwrap(),
            capability_hash(&right).unwrap()
        );
        assert_ne!(
            capability_hash(&left).unwrap(),
            capability_hash(&json!({"a":{"x":1,"y":2},"b":[2,1]})).unwrap()
        );
        let mut schema = ToolSchema::function("first", "description", json!({"type":"object"}));
        let original = capability_hash(&schema).unwrap();
        schema.function.description.push('!');
        assert_ne!(capability_hash(&schema).unwrap(), original);
    }

    #[test]
    fn h219_schema_changes_are_validated_and_generation_only_commits_after_persistence() {
        let (db, registry, tools) = fixture();
        let recorder = recorder(&db, &registry, &tools[..1]);
        recorder.record_model_tools(&tools[..1]).unwrap();
        for invalid in [
            vec![ToolSchema::function(
                "unknown",
                "",
                json!({"type":"object"}),
            )],
            vec![tools[0].clone(), tools[0].clone()],
            vec![ToolSchema::function(
                "first",
                "changed descriptor",
                json!({"type":"object"}),
            )],
        ] {
            assert!(recorder.record_model_tools(&invalid).is_err());
        }
        db.with_conn(|conn| conn.execute_batch("CREATE TRIGGER reject_schema BEFORE INSERT ON run_events WHEN NEW.event_type='capability_schema_snapshot' BEGIN SELECT RAISE(ABORT, 'synthetic schema failure'); END;").map_err(|e| CoreError::Persistence(e.to_string()))).unwrap();
        assert!(recorder.record_model_tools(&tools).is_err());
        let before = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        assert_eq!(before.len(), 2); // Admission and the initial snapshot only.
        db.with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_schema;")
                .map_err(|e| CoreError::Persistence(e.to_string()))
        })
        .unwrap();
        recorder.record_model_tools(&tools).unwrap();
        recorder.record_model_tools(&tools).unwrap();
        let events = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[2].data["modelSchema"]["generation"], 1);
        assert_eq!(events[2].data["snapshotId"], events[1].data["snapshotId"]);
        assert!(!serde_json::to_string(&events)
            .unwrap()
            .contains("PRIVATE_SCHEMA_DESCRIPTION_NOT_FOR_LOGS"));
        assert_eq!(
            events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn h219_terminal_model_observer_rejects_unchanged_and_changed_schemas() {
        let (db, registry, tools) = fixture();
        let recorder = recorder(&db, &registry, &tools[..1]);
        recorder.record_model_tools(&tools[..1]).unwrap();
        RunStore::new(&db)
            .finish_with_event(
                "h219-unit",
                "cancelled",
                None,
                &json!({"kind":"cancelled"}),
                None,
                2,
            )
            .unwrap();
        let before = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        let accepted = [
            recorder.record_model_tools(&tools[..1]).is_ok(),
            recorder.record_model_tools(&tools).is_ok(),
        ];
        assert_eq!(
            accepted,
            [false, false],
            "a terminal owner cannot authorize another schema read"
        );
        assert_eq!(
            RunStore::new(&db).events_after("h219-unit", None).unwrap(),
            before
        );
        assert_eq!(recorder.state.lock().unwrap().number, 0);
    }

    #[test]
    fn h219_terminal_wire_observer_rejects_first_duplicate_and_changed_routes() {
        for previously_observed in [false, true] {
            let (db, registry, tools) = fixture();
            let recorder = recorder(&db, &registry, &tools);
            let client = deepagent_models::ModelClient::new(
                Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
                deepagent_models::ModelConfig::deepseek("synthetic-placeholder"),
            );
            let wire = client.project_tool_schemas("primary", &tools);
            if previously_observed {
                recorder.record_wire_tools(&tools, &wire).unwrap();
            }
            RunStore::new(&db)
                .finish_with_event(
                    "h219-unit",
                    "succeeded",
                    None,
                    &json!({"kind":"succeeded"}),
                    None,
                    2,
                )
                .unwrap();
            let before = RunStore::new(&db).events_after("h219-unit", None).unwrap();
            let fallback = client.project_tool_schemas("fallback", &tools);
            let accepted = [
                recorder.record_wire_tools(&tools, &wire).is_ok(),
                recorder.record_wire_tools(&tools, &fallback).is_ok(),
            ];
            assert_eq!(
                accepted,
                [false, false],
                "previously_observed={previously_observed}"
            );
            assert_eq!(
                RunStore::new(&db).events_after("h219-unit", None).unwrap(),
                before
            );
            assert_eq!(
                recorder.wire_state.lock().unwrap().is_some(),
                previously_observed
            );
        }
    }

    #[test]
    fn h219_concurrent_same_schema_publish_has_one_durable_generation() {
        let (db, registry, tools) = fixture();
        let recorder = Arc::new(recorder(&db, &registry, &tools[..1]));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let threads = (0..16)
            .map(|_| {
                let recorder = recorder.clone();
                let tools = tools.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    recorder.record_model_tools(&tools).unwrap();
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        let events = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[2].data["modelSchema"]["generation"], 1);
    }

    #[test]
    fn h219_resolution_is_read_only_and_binding_has_distinct_run_identity() {
        let (db, registry, tools) = fixture();
        let resolve = || {
            ResolvedRunCapabilities::resolve(
                json!({}),
                &registry,
                &tools,
                ApprovalPolicy::AlwaysAsk,
            )
            .unwrap()
        };
        let first = resolve();
        let second = resolve();
        assert_eq!(first.capability_hash, second.capability_hash);
        assert_eq!(
            RunStore::new(&db)
                .events_after("h219-unit", None)
                .unwrap()
                .len(),
            1
        );
        first.bind(db.clone(), "h219-unit").unwrap();
        let session = RunStore::new(&db)
            .get("h219-unit")
            .unwrap()
            .unwrap()
            .session_id;
        RunStore::new(&db)
            .create("h219-other", &session, None, 1)
            .unwrap();
        second.bind(db.clone(), "h219-other").unwrap();
        let first = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        let second = RunStore::new(&db).events_after("h219-other", None).unwrap();
        assert_eq!(
            first[1].data["capabilityHash"],
            second[1].data["capabilityHash"]
        );
        assert_ne!(first[1].data["snapshotId"], second[1].data["snapshotId"]);
    }

    #[test]
    fn h219_concurrent_initial_bind_accepts_exactly_one_snapshot() {
        let (db, registry, tools) = fixture();
        let candidates = (0..16)
            .map(|_| {
                ResolvedRunCapabilities::resolve(
                    json!({}),
                    &registry,
                    &tools,
                    ApprovalPolicy::AlwaysAsk,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let barrier = Arc::new(std::sync::Barrier::new(candidates.len()));
        let threads = candidates
            .into_iter()
            .map(|candidate| {
                let db = db.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    candidate.bind(db, "h219-unit").is_ok()
                })
            })
            .collect::<Vec<_>>();
        let accepted = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|accepted| *accepted)
            .count();
        assert_eq!(
            accepted, 1,
            "an admitted run must have one immutable capability snapshot"
        );
        let events = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "capability.snapshot")
                .count(),
            1
        );
    }

    #[test]
    fn h219_context_tracks_effective_profile_rules_and_workspace_without_raw_rules() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let make = |root, profile, permission_rules: &deepagent_hooks::PermissionRules| {
            capability_context(CapabilityContextRequest {
                root,
                profile,
                permission_rules,
                sandbox_backend: Some(SandboxBackendKind::Direct),
                backends: None,
                tool_search_mode: deepagent_builtins::ToolSearchMode::Disabled,
                tool_search_threshold: 100,
                mcp_lifecycle: &[],
            })
            .unwrap()
        };
        let profile = crate::settings::PermissionPreset::FullAccess.to_effective_profile();
        let rules = deepagent_hooks::PermissionRules::default();
        let original = make(first.path(), profile, &rules);
        let other_workspace = make(second.path(), profile, &rules);
        assert_ne!(
            capability_hash(&original).unwrap(),
            capability_hash(&other_workspace).unwrap()
        );
        let restricted = make(
            first.path(),
            EffectivePermissionProfile {
                sandbox_mode: crate::settings::SandboxMode::ReadOnly,
                ..profile
            },
            &rules,
        );
        assert_ne!(
            original["permissionProfileHash"],
            restricted["permissionProfileHash"]
        );
        let rule = "Read(PRIVATE_RULE_PATH)";
        let other_rules = make(
            first.path(),
            profile,
            &deepagent_hooks::PermissionRules {
                deny: vec![rule.into()],
                ..Default::default()
            },
        );
        assert_ne!(
            original["permissionRulesHash"],
            other_rules["permissionRulesHash"]
        );
        assert!(!other_rules.to_string().contains(rule));
        assert_eq!(original["sandboxCapabilities"]["kind"], "direct");
    }
}
