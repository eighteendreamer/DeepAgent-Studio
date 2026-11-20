//! Storage layout for the existing coordinator, not another queue or scheduler.
//! Each live task reserves its entire bounded row before execution. A completed
//! task is charged its actual serialized bytes; history cannot consume a live
//! task's reservation. The index and changed/deleted rows commit in one CAS.
use super::{invalid, Document, TransferTask, COLLECTION, LIMIT};
use deepagent_core::{clock::Timestamp, error::Result};
use deepagent_persistence::{
    document_store::{DocumentBodyChange, DocumentBodyDelete, DocumentStore},
    Database,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const TASK_BYTES: usize = 1024 * 1024;
const HISTORY_BYTES: usize = 16 * TASK_BYTES;
const INDEX_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskReference {
    id: String,
    revision: u64,
    bytes: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    revision: u64,
    tasks: Vec<TaskReference>,
}

#[derive(Default)]
pub(super) struct QueueStorage {
    index_body: Option<String>,
    revision: u64,
    task_bodies: HashMap<String, String>,
}

fn validate_ids<'a>(ids: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = HashSet::new();
    for id in ids {
        if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) || !seen.insert(id) {
            return Err(invalid("invalid or duplicate transfer task id"));
        }
    }
    if seen.len() > LIMIT {
        return Err(invalid("transfer history is full; clear finished tasks"));
    }
    Ok(())
}

fn task_key(id: &str) -> String {
    format!("task:{id}")
}

fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T> {
    serde_json::from_str(body).map_err(|_| invalid("invalid transfer queue document"))
}

fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| invalid("transfer serialization failed"))
}

impl QueueStorage {
    pub(super) fn load(db: &Database) -> Result<(Document, Self)> {
        let store = DocumentStore::new(db);
        // The original index contains the complete v1 history, capped at 1 MiB.
        let index_body = store.get_body_bounded(COLLECTION, "queue", TASK_BYTES)?;
        let mut storage = Self {
            index_body,
            ..Self::default()
        };
        let Some(body) = storage.index_body.as_deref() else {
            return Ok((
                Document {
                    version: 1,
                    tasks: vec![],
                },
                storage,
            ));
        };
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let document = match parse::<Version>(body)?.version {
            1 => {
                let document: Document = parse(body)?;
                validate_ids(document.tasks.iter().map(|task| task.id.as_str()))?;
                document
            }
            2 => {
                if body.len() > INDEX_BYTES {
                    return Err(invalid("transfer index exceeds limit"));
                }
                let manifest: Manifest = parse(body)?;
                if manifest.revision == 0 {
                    return Err(invalid("invalid transfer index revision"));
                }
                validate_ids(manifest.tasks.iter().map(|task| task.id.as_str()))?;
                let mut actual_bytes = 0usize;
                for reference in &manifest.tasks {
                    if reference.bytes == 0 || reference.bytes > TASK_BYTES {
                        return Err(invalid("transfer task document exceeds limit"));
                    }
                    actual_bytes += reference.bytes;
                }
                if actual_bytes > HISTORY_BYTES {
                    return Err(invalid("transfer history capacity exceeded"));
                }
                let mut tasks = Vec::with_capacity(manifest.tasks.len());
                let mut reserved_bytes = 0;
                for reference in manifest.tasks {
                    let key = task_key(&reference.id);
                    let body = store
                        .get_body_bounded(COLLECTION, &key, TASK_BYTES)?
                        .ok_or_else(|| invalid("missing transfer task document"))?;
                    let task: TransferTask = parse(&body)?;
                    if task.id != reference.id
                        || task.revision != reference.revision
                        || body.len() != reference.bytes
                    {
                        return Err(invalid("transfer task reference changed; reload host"));
                    }
                    reserved_bytes += if task.state.terminal() {
                        body.len()
                    } else {
                        TASK_BYTES
                    };
                    storage.task_bodies.insert(key, body);
                    tasks.push(task);
                }
                if reserved_bytes > HISTORY_BYTES {
                    return Err(invalid("transfer history capacity exceeded"));
                }
                storage.revision = manifest.revision;
                Document { version: 1, tasks }
            }
            _ => return Err(invalid("unsupported transfer queue version")),
        };
        // Reads use the existing store. The monotonic index fences a concurrent
        // transaction across the individual row reads, including clear/reinsert.
        if store.get_body_bounded(COLLECTION, "queue", TASK_BYTES)? != storage.index_body {
            return Err(invalid(
                "transfer coordinator ownership changed; reload host",
            ));
        }
        Ok((document, storage))
    }

    pub(super) fn save(
        &mut self,
        db: &Database,
        tasks: &[TransferTask],
        now: Timestamp,
    ) -> Result<()> {
        validate_ids(tasks.iter().map(|task| task.id.as_str()))?;
        let mut bodies = HashMap::with_capacity(tasks.len());
        let mut references = Vec::with_capacity(tasks.len());
        let mut reserved_bytes = 0;
        for task in tasks {
            let body = encode(task)?;
            if body.len() > TASK_BYTES {
                return Err(invalid("transfer task document exceeds limit"));
            }
            reserved_bytes += if task.state.terminal() {
                body.len()
            } else {
                TASK_BYTES
            };
            if reserved_bytes > HISTORY_BYTES {
                return Err(invalid(
                    "transfer history capacity exceeded; clear finished tasks",
                ));
            }
            references.push(TaskReference {
                id: task.id.clone(),
                revision: task.revision,
                bytes: body.len(),
            });
            bodies.insert(task_key(&task.id), body);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("transfer index revision limit reached"))?;
        let index_body = encode(&Manifest {
            version: 2,
            revision,
            tasks: references,
        })?;
        if index_body.len() > INDEX_BYTES {
            return Err(invalid("transfer index exceeds limit"));
        }
        let mut changes = vec![DocumentBodyChange {
            collection: COLLECTION,
            id: "queue",
            expected: self.index_body.as_deref(),
            body: &index_body,
        }];
        for (key, body) in &bodies {
            let expected = self.task_bodies.get(key).map(String::as_str);
            if expected != Some(body.as_str()) {
                changes.push(DocumentBodyChange {
                    collection: COLLECTION,
                    id: key,
                    expected,
                    body,
                });
            }
        }
        let deletes: Vec<_> = self
            .task_bodies
            .iter()
            .filter(|(key, _)| !bodies.contains_key(*key))
            .map(|(key, body)| DocumentBodyDelete {
                collection: COLLECTION,
                id: key,
                expected: body,
            })
            .collect();
        if !DocumentStore::new(db).compare_exchange_bodies_and_delete(&changes, &deletes, now)? {
            return Err(invalid(
                "transfer coordinator ownership changed; reload host",
            ));
        }
        self.index_body = Some(index_body);
        self.revision = revision;
        self.task_bodies = bodies;
        Ok(())
    }
}
