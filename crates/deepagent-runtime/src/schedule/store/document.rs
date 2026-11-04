//! One lossless editing boundary for the existing schedule file. A usable
//! projection is not a replacement document: invalid rows and unknown values
//! remain raw JSON, including numbers outside serde_json::Value's integer range.

use super::*;
use serde::de::{MapAccess, Visitor};
use serde_json::value::{to_raw_value, RawValue};
use std::collections::{BTreeMap, HashSet};

#[derive(Default, Serialize)]
#[serde(transparent)]
struct RawFields(BTreeMap<String, Box<RawValue>>);

impl<'de> Deserialize<'de> for RawFields {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct UniqueFields;
        impl<'de> Visitor<'de> for UniqueFields {
            type Value = RawFields;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object with unique field names")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut input: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = input.next_entry::<String, Box<RawValue>>()? {
                    if fields.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate schedule field"));
                    }
                }
                Ok(RawFields(fields))
            }
        }
        deserializer.deserialize_map(UniqueFields)
    }
}

struct TaskRow {
    raw: Box<RawValue>,
    task: Option<ScheduledTask>,
}

impl TaskRow {
    fn new(task: ScheduledTask) -> Result<Self> {
        Ok(Self {
            raw: to_raw_value(&task).map_err(encode_error)?,
            task: Some(task),
        })
    }

    fn replace(&mut self, task: ScheduledTask) -> Result<()> {
        let mut fields: RawFields = serde_json::from_str(self.raw.get()).map_err(encode_error)?;
        // Only these fields are owned by ScheduledTask. In particular, absent
        // optional fields must be removed without dropping future extensions.
        for key in [
            "id",
            "cron",
            "prompt",
            "createdAt",
            "lastFiredAt",
            "recurring",
            "permanent",
        ] {
            fields.0.remove(key);
        }
        let known: RawFields =
            serde_json::from_str(to_raw_value(&task).map_err(encode_error)?.get())
                .map_err(encode_error)?;
        fields.0.extend(known.0);
        self.raw = to_raw_value(&fields).map_err(encode_error)?;
        self.task = Some(task);
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct CronDocument {
    fields: RawFields,
    rows: Vec<TaskRow>,
}

impl CronDocument {
    pub(super) fn read(path: &Path) -> Result<Self> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => {
                return Err(CoreError::other(format!(
                    "failed to read {}: {error}",
                    path.display()
                )))
            }
        };
        let mut fields: RawFields = serde_json::from_str(&raw).map_err(|error| {
            CoreError::invalid(format!("invalid scheduled-tasks document at {} (line {}, column {}); repair before writing", path.display(), error.line(), error.column()))
        })?;
        let tasks = fields.0.remove("tasks").ok_or_else(document_error)?;
        let raw_rows: Vec<Box<RawValue>> =
            serde_json::from_str(tasks.get()).map_err(|_| document_error())?;
        let mut rows = Vec::with_capacity(raw_rows.len());
        let mut rejected = 0usize;
        for (index, raw) in raw_rows.into_iter().enumerate() {
            let task = match decode_task(&raw) {
                Ok(task) => Some(task),
                Err(reason) => {
                    rejected += 1;
                    tracing::warn!(path = %path.display(), row_index = index, reason, "invalid scheduled-task row retained but excluded from dispatch");
                    None
                }
            };
            rows.push(TaskRow { raw, task });
        }
        if rejected > 0 {
            tracing::warn!(path = %path.display(), total_rows = rows.len(), rejected_rows = rejected, "loaded partial scheduled-task projection; edits preserve unrecognized rows");
        }
        Ok(Self { fields, rows })
    }

    pub(super) fn tasks(&self) -> Vec<ScheduledTask> {
        self.rows
            .iter()
            .filter_map(|row| row.task.clone())
            .collect()
    }

    pub(super) fn push(&mut self, task: ScheduledTask) -> Result<()> {
        self.rows.push(TaskRow::new(task)?);
        Ok(())
    }

    pub(super) fn remove(&mut self, ids: &[String]) -> usize {
        let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let before = self.rows.len();
        self.rows.retain(|row| {
            row.task
                .as_ref()
                .map_or(true, |task| !ids.contains(task.id.as_str()))
        });
        before - self.rows.len()
    }

    pub(super) fn mark_fired(&mut self, ids: &[String], at: i64) -> Result<usize> {
        let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let mut changed = 0;
        for row in &mut self.rows {
            if let Some(task) = row
                .task
                .as_ref()
                .filter(|task| ids.contains(task.id.as_str()))
            {
                let mut task = task.clone();
                task.last_fired_at = Some(at);
                row.replace(task)?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    pub(super) fn replace(&mut self, tasks: &[ScheduledTask]) -> Result<()> {
        let mut desired = HashSet::new();
        for task in tasks {
            if !desired.insert(task.id.as_str()) {
                return Err(identity_error());
            }
        }
        let mut existing = BTreeMap::new();
        let mut opaque = Vec::new();
        for row in std::mem::take(&mut self.rows) {
            if let Some(task) = &row.task {
                if existing.insert(task.id.clone(), row).is_some() {
                    return Err(identity_error());
                }
            } else {
                opaque.push(row);
            }
        }
        let mut rows = Vec::with_capacity(tasks.len() + opaque.len());
        // The public replacement API has always preserved its input order.
        // Retain extensions by identity, not by the row's previous position.
        for task in tasks {
            if let Some(mut row) = existing.remove(&task.id) {
                row.replace(task.clone())?;
                rows.push(row);
            } else {
                rows.push(TaskRow::new(task.clone())?);
            }
        }
        rows.extend(opaque);
        self.rows = rows;
        Ok(())
    }

    pub(super) fn encode(mut self) -> Result<String> {
        let rows: Vec<&RawValue> = self.rows.iter().map(|row| row.raw.as_ref()).collect();
        self.fields
            .0
            .insert("tasks".into(), to_raw_value(&rows).map_err(encode_error)?);
        let json = serde_json::to_string_pretty(&self.fields).map_err(encode_error)?;
        Ok(format!("{json}\n"))
    }
}

fn decode_task(raw: &RawValue) -> std::result::Result<ScheduledTask, &'static str> {
    serde_json::from_str::<RawFields>(raw.get()).map_err(|_| "object_or_duplicate_field")?;
    let task: ScheduledTask = serde_json::from_str(raw.get()).map_err(|_| "schema")?;
    if task.id.trim().is_empty() || task.cron.trim().is_empty() || task.prompt.trim().is_empty() {
        return Err("required_field");
    }
    if crate::schedule::cron::parse_5_field(&task.cron).is_err() {
        return Err("cron");
    }
    Ok(task)
}

fn encode_error(_: serde_json::Error) -> CoreError {
    CoreError::invalid("cannot encode scheduled-task edit; original file was not replaced")
}

fn document_error() -> CoreError {
    CoreError::invalid(
        "scheduled-tasks document requires an object with a tasks array; repair before writing",
    )
}

fn identity_error() -> CoreError {
    CoreError::invalid("ambiguous scheduled-task identity; refusing projection replacement")
}
