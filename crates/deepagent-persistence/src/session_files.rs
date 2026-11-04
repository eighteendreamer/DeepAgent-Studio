use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use deepagent_core::clock::Timestamp;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::Event;
use deepagent_core::id::SessionId;
use deepagent_core::session_mode::SessionMode;
use fs2::FileExt;
use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u32 = 1;
const FILE_SUFFIX: &str = ".v1.jsonl.zst";
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const SESSION_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const SESSION_LOCK_POLL: Duration = Duration::from_millis(10);

pub(crate) mod page;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionFileHeader {
    pub(crate) id: SessionId,
    pub(crate) title: Option<String>,
    pub(crate) mode: SessionMode,
    pub(crate) project: Option<String>,
    pub(crate) created_at: Timestamp,
    /// Absent in older/non-harness sessions; never inferred from transcript text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) execution_config: Option<deepagent_core::execution_config::SessionExecutionConfig>,
    /// Only new sessions have a known initial inactive plan mode. Legacy
    /// headers remain unknown until an explicit PlanModeChanged selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plan_mode_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum StoredRecord {
    Header {
        format: u32,
        generation: u64,
        session: SessionFileHeader,
    },
    Event {
        schema: u32,
        event: Event,
    },
    /// One source and its complete synthetic/native-only projection decision.
    /// The whole record occupies one checksummed frame, never a partial group.
    Projection {
        schema: u32,
        source: Event,
        items: Vec<Event>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProjectionSpan {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug)]
pub(crate) struct LoadedSessionFile {
    pub(crate) generation: u64,
    pub(crate) header: SessionFileHeader,
    pub(crate) events: Vec<Event>,
    pub(crate) projections: Vec<ProjectionSpan>,
    pub(crate) byte_len: u64,
    pub(crate) recovered_tail_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SessionFileCursor {
    pub(crate) generation: u64,
    pub(crate) last_sequence: Option<u64>,
    pub(crate) byte_len: u64,
}

#[derive(Debug)]
pub(crate) struct AppendedSessionEvent {
    pub(crate) event: Event,
    pub(crate) items: Vec<Event>,
    pub(crate) generation: u64,
    pub(crate) byte_len: u64,
    pub(crate) recovered_tail_bytes: u64,
    pub(crate) rebuild_events: Option<Vec<Event>>,
}

pub(crate) struct SessionLock {
    file: File,
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(crate) fn create(root: &Path, header: SessionFileHeader) -> Result<LoadedSessionFile> {
    let generation = 1;
    let dir = session_dir(root, header.id);
    fs::create_dir_all(&dir).map_err(io_error)?;
    let path = generation_path(&dir, generation);
    if path.exists() {
        return Err(CoreError::Persistence(format!(
            "session file already exists: {}",
            path.display()
        )));
    }

    let byte_len = write_generation(&path, generation, &header, &[], &[])?;
    Ok(LoadedSessionFile {
        generation,
        header,
        events: Vec::new(),
        projections: Vec::new(),
        byte_len,
        recovered_tail_bytes: 0,
    })
}

pub(crate) fn load(root: &Path, session_id: SessionId) -> Result<Option<LoadedSessionFile>> {
    let Some((generation, path)) = current_generation(root, session_id)? else {
        return Ok(None);
    };
    let mut file = open_session_file(&path, false)?;
    lock_exclusive_with_timeout(&file)?;
    let result = read_locked_file(&mut file, generation, session_id, true);
    let unlock_result = FileExt::unlock(&file).map_err(io_error);
    match (result, unlock_result) {
        (Ok(loaded), Ok(())) => Ok(Some(loaded)),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

/// Prepare a complete fork prefix without publishing a partially-created
/// session. The caller holds the new session lock and publishes this directory
/// only after all metadata/index writes have succeeded in its transaction.
pub(crate) fn prepare_fork(
    root: &Path,
    header: SessionFileHeader,
    events: Vec<Event>,
    projections: Vec<ProjectionSpan>,
) -> Result<(tempfile::TempDir, LoadedSessionFile)> {
    let target = session_dir(root, header.id);
    let parent = target
        .parent()
        .ok_or_else(|| CoreError::invalid("session path has no parent"))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let staging = tempfile::Builder::new()
        .prefix(".fork-")
        .tempdir_in(parent)
        .map_err(io_error)?;
    let generation = 1;
    let byte_len = write_generation(
        &generation_path(staging.path(), generation),
        generation,
        &header,
        &events,
        &projections,
    )?;
    Ok((
        staging,
        LoadedSessionFile {
            generation,
            header,
            events,
            projections,
            byte_len,
            recovered_tail_bytes: 0,
        },
    ))
}

pub(crate) fn current_cursor(
    root: &Path,
    session_id: SessionId,
) -> Result<Option<SessionFileCursor>> {
    let Some((generation, path)) = current_generation(root, session_id)? else {
        return Ok(None);
    };
    Ok(Some(SessionFileCursor {
        generation,
        last_sequence: None,
        byte_len: fs::metadata(path).map_err(io_error)?.len(),
    }))
}

pub(crate) fn list_active_session_ids(root: &Path) -> Result<Vec<SessionId>> {
    let shards = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(error)),
    };
    let mut ids = Vec::new();
    for shard in shards {
        let shard = shard.map_err(io_error)?;
        if !shard.file_type().map_err(io_error)?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(shard.path()).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if !entry.file_type().map_err(io_error)?.is_dir() {
                continue;
            }
            let Ok(id) = entry.file_name().to_string_lossy().parse::<SessionId>() else {
                continue;
            };
            if entry.path() == session_dir(root, id) && current_generation(root, id)?.is_some() {
                ids.push(id);
            }
        }
    }
    ids.sort_unstable_by_key(ToString::to_string);
    Ok(ids)
}

#[cfg(test)]
pub(crate) fn append(
    root: &Path,
    session_id: SessionId,
    event: Event,
    cursor: Option<SessionFileCursor>,
) -> Result<AppendedSessionEvent> {
    append_projection(root, session_id, event, None, cursor)
}

pub(crate) fn append_projection(
    root: &Path,
    session_id: SessionId,
    mut event: Event,
    projected_items: Option<Vec<deepagent_core::response_item::ResponseItem>>,
    cursor: Option<SessionFileCursor>,
) -> Result<AppendedSessionEvent> {
    let (generation, path) = current_generation(root, session_id)?.ok_or_else(|| {
        CoreError::EventLog(format!("session {session_id} has no persisted event file"))
    })?;
    let mut file = open_session_file(&path, true).map_err(|error| {
        CoreError::Persistence(format!("open session file {}: {error}", path.display()))
    })?;
    lock_exclusive_with_timeout(&file)?;

    let file_len = file
        .metadata()
        .map_err(|error| CoreError::Persistence(format!("stat session file: {error}")))?
        .len();
    let matching_cursor =
        cursor.filter(|cursor| cursor.generation == generation && cursor.byte_len == file_len);
    let needs_rebuild = matching_cursor.is_none();
    let recovered_tail_bytes;
    event.sequence = if let Some(cursor) = matching_cursor {
        recovered_tail_bytes = 0;
        cursor.last_sequence.map_or(0, |sequence| sequence + 1)
    } else {
        let loaded = read_locked_file(&mut file, generation, session_id, true)?;
        recovered_tail_bytes = loaded.recovered_tail_bytes;
        loaded.events.len() as u64
    };

    let items: Vec<Event> = projected_items
        .as_deref()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(index, item)| Event {
            id: deepagent_core::id::EventId::new(),
            session_id,
            sequence: event.sequence + index as u64 + 1,
            timestamp: event.timestamp,
            payload: deepagent_core::event::EventPayload::ResponseItemAppended {
                item: item.clone(),
            },
        })
        .collect();
    let record = if projected_items.is_some() {
        StoredRecord::Projection {
            schema: FORMAT_VERSION,
            source: event.clone(),
            items: items.clone(),
        }
    } else {
        StoredRecord::Event {
            schema: FORMAT_VERSION,
            event: event.clone(),
        }
    };
    let encoded = encode_records(std::slice::from_ref(&record))?;
    file.seek(SeekFrom::End(0))
        .map_err(|error| CoreError::Persistence(format!("seek session file: {error}")))?;
    file.write_all(&encoded)
        .map_err(|error| CoreError::Persistence(format!("write session file: {error}")))?;
    file.sync_all()
        .map_err(|error| CoreError::Persistence(format!("sync session file: {error}")))?;
    let byte_len = file
        .metadata()
        .map_err(|error| CoreError::Persistence(format!("stat session file: {error}")))?
        .len();
    let rebuild_events = if needs_rebuild {
        Some(read_locked_file(&mut file, generation, session_id, false)?.events)
    } else {
        None
    };
    FileExt::unlock(&file)
        .map_err(|error| CoreError::Persistence(format!("unlock session file: {error}")))?;
    Ok(AppendedSessionEvent {
        event,
        items,
        generation,
        byte_len,
        recovered_tail_bytes,
        rebuild_events,
    })
}

pub(crate) fn rewrite_prefix(
    root: &Path,
    session_id: SessionId,
    keep_through: u64,
) -> Result<(LoadedSessionFile, u64)> {
    let current = load(root, session_id)?.ok_or_else(|| {
        CoreError::EventLog(format!("session {session_id} has no persisted event file"))
    })?;
    let projections = crate::event_projection::projection_spans(
        &current.events,
        &current.projections,
        Some(keep_through),
    )?;
    let keep_through = crate::event_projection::complete_boundary(keep_through, &projections);
    let kept: Vec<Event> = current
        .events
        .iter()
        .filter(|event| event.sequence <= keep_through)
        .cloned()
        .collect();
    let removed = current.events.len().saturating_sub(kept.len()) as u64;
    let projections: Vec<_> = projections
        .into_iter()
        .filter(|span| span.end <= keep_through)
        .collect();
    let generation = current.generation + 1;
    let dir = session_dir(root, session_id);
    let path = generation_path(&dir, generation);
    let byte_len = write_generation(&path, generation, &current.header, &kept, &projections)?;
    Ok((
        LoadedSessionFile {
            generation,
            header: current.header,
            events: kept,
            projections,
            byte_len,
            recovered_tail_bytes: 0,
        },
        removed,
    ))
}

/// Re-stamp the header's project and keep the event stream byte-identical.
///
/// The header is record zero of the compressed stream, so the only way to edit
/// it is to re-emit the file as a new generation. Callers must move the
/// `session_file_state` cursor afterwards, exactly like [`rewrite_prefix`].
pub(crate) fn rewrite_header_project(
    root: &Path,
    session_id: SessionId,
    project: &str,
) -> Result<LoadedSessionFile> {
    let current = load(root, session_id)?.ok_or_else(|| {
        CoreError::EventLog(format!("session {session_id} has no persisted event file"))
    })?;
    let generation = current.generation + 1;
    let mut header = current.header.clone();
    header.project = Some(project.to_string());
    let path = generation_path(&session_dir(root, session_id), generation);
    let byte_len = write_generation(
        &path,
        generation,
        &header,
        &current.events,
        &current.projections,
    )?;
    Ok(LoadedSessionFile {
        generation,
        header,
        events: current.events,
        projections: current.projections,
        byte_len,
        recovered_tail_bytes: 0,
    })
}

pub(crate) fn prune_older_generations(
    root: &Path,
    session_id: SessionId,
    current_generation: u64,
) -> Result<()> {
    let dir = session_dir(root, session_id);
    for entry in fs::read_dir(&dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(generation) = name
            .strip_prefix("session.g")
            .and_then(|rest| rest.strip_suffix(FILE_SUFFIX))
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        if generation < current_generation {
            fs::remove_file(entry.path()).map_err(io_error)?;
        }
    }
    Ok(())
}

pub(crate) fn move_to_trash(root: &Path, session_id: SessionId, deleted_at: i64) -> Result<bool> {
    let source = session_dir(root, session_id);
    if !source.exists() {
        return Ok(false);
    }
    let destination = trash_session_root(root, session_id).join(deleted_at.to_string());
    if destination.exists() {
        return Err(CoreError::Persistence(format!(
            "session trash destination already exists: {}",
            destination.display()
        )));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| CoreError::Persistence("trash path has no parent".into()))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    fs::rename(&source, &destination).map_err(io_error)?;
    Ok(true)
}

pub(crate) fn restore_from_trash(root: &Path, session_id: SessionId) -> Result<bool> {
    let destination = session_dir(root, session_id);
    if destination.exists() {
        return Ok(false);
    }
    let trash_root = trash_session_root(root, session_id);
    let entries = match fs::read_dir(&trash_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error(error)),
    };
    let mut selected: Option<(i64, PathBuf)> = None;
    for entry in entries {
        let entry = entry.map_err(io_error)?;
        let Some(deleted_at) = entry.file_name().to_string_lossy().parse::<i64>().ok() else {
            continue;
        };
        if selected
            .as_ref()
            .map_or(true, |(current, _)| deleted_at > *current)
        {
            selected = Some((deleted_at, entry.path()));
        }
    }
    let Some((_, source)) = selected else {
        return Ok(false);
    };
    let parent = destination
        .parent()
        .ok_or_else(|| CoreError::Persistence("session path has no parent".into()))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    fs::rename(source, destination).map_err(io_error)?;
    let _ = fs::remove_dir(&trash_root);
    Ok(true)
}

pub(crate) fn latest_trash_timestamp(root: &Path, session_id: SessionId) -> Result<Option<i64>> {
    let trash_root = trash_session_root(root, session_id);
    let entries = match fs::read_dir(trash_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let mut latest: Option<i64> = None;
    for entry in entries {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_dir() {
            continue;
        }
        let Some(timestamp) = entry.file_name().to_string_lossy().parse::<i64>().ok() else {
            continue;
        };
        latest = Some(latest.map_or(timestamp, |current| current.max(timestamp)));
    }
    Ok(latest)
}

pub(crate) fn purge_trash_before(root: &Path, cutoff_ms: i64) -> Result<u64> {
    let trash_root = root
        .parent()
        .ok_or_else(|| CoreError::Persistence("session root has no parent".into()))?
        .join("trash")
        .join("sessions");
    let sessions = match fs::read_dir(&trash_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(io_error(error)),
    };
    let mut removed = 0u64;
    for session in sessions {
        let session = session.map_err(io_error)?;
        if !session.file_type().map_err(io_error)?.is_dir() {
            continue;
        }
        let Ok(session_id) = session.file_name().to_string_lossy().parse::<SessionId>() else {
            continue;
        };
        let _session_lock = lock_session(root, session_id)?;
        for generation in fs::read_dir(session.path()).map_err(io_error)? {
            let generation = generation.map_err(io_error)?;
            let Some(deleted_at) = generation.file_name().to_string_lossy().parse::<i64>().ok()
            else {
                continue;
            };
            if deleted_at <= cutoff_ms && generation.file_type().map_err(io_error)?.is_dir() {
                fs::remove_dir_all(generation.path()).map_err(io_error)?;
                removed += 1;
            }
        }
        let _ = fs::remove_dir(session.path());
    }
    let _ = fs::remove_dir(&trash_root);
    Ok(removed)
}

fn read_locked_file(
    file: &mut File,
    generation: u64,
    expected_session: SessionId,
    repair_tail: bool,
) -> Result<LoadedSessionFile> {
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_error)?;
    let (records, valid_len, torn_tail) = decode_frames(&bytes)?;
    if torn_tail > 0 && repair_tail {
        file.set_len(valid_len as u64).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
    }
    parse_records(
        records,
        generation,
        expected_session,
        valid_len as u64,
        torn_tail as u64,
    )
}

fn parse_records(
    records: Vec<StoredRecord>,
    generation: u64,
    expected_session: SessionId,
    byte_len: u64,
    recovered_tail_bytes: u64,
) -> Result<LoadedSessionFile> {
    let mut records = records.into_iter();
    let header = parse_header(records.next(), generation, expected_session)?;
    let mut events = Vec::new();
    let mut projections = Vec::new();
    for record in records {
        let (group, projection) = parse_event_group(record, expected_session, events.len() as u64)?;
        projections.extend(projection);
        events.extend(group);
    }
    Ok(LoadedSessionFile {
        generation,
        header,
        events,
        projections,
        byte_len,
        recovered_tail_bytes,
    })
}

fn parse_header(
    record: Option<StoredRecord>,
    generation: u64,
    expected_session: SessionId,
) -> Result<SessionFileHeader> {
    let header = match record {
        Some(StoredRecord::Header {
            format,
            generation: stored_generation,
            session,
        }) if format == FORMAT_VERSION && stored_generation == generation => session,
        Some(_) => {
            return Err(CoreError::EventLog(format!(
                "invalid session header for {expected_session} generation {generation}"
            )))
        }
        None => {
            return Err(CoreError::EventLog(format!(
                "empty session file for {expected_session}"
            )))
        }
    };
    if header.id != expected_session {
        return Err(CoreError::EventLog(format!(
            "session header id {} does not match path id {expected_session}",
            header.id
        )));
    }

    Ok(header)
}

/// Both replay and bounded history use this one schema/identity/sequence gate.
fn parse_event_group(
    record: StoredRecord,
    expected_session: SessionId,
    mut expected_sequence: u64,
) -> Result<(Vec<Event>, Option<ProjectionSpan>)> {
    let mut projection = None;
    let group: Vec<Event> = match record {
        StoredRecord::Event { schema, event } if schema == FORMAT_VERSION => vec![event],
        StoredRecord::Projection {
            schema,
            source,
            items,
        } if schema == FORMAT_VERSION => {
            if items.iter().any(|event| {
                !matches!(
                    event.payload,
                    deepagent_core::event::EventPayload::ResponseItemAppended { .. }
                )
            }) {
                return Err(CoreError::EventLog(
                    "projection contains a non-native item".into(),
                ));
            }
            projection = Some(ProjectionSpan {
                start: source.sequence,
                end: source
                    .sequence
                    .checked_add(items.len() as u64)
                    .ok_or_else(|| CoreError::EventLog("projection sequence overflow".into()))?,
            });
            std::iter::once(source).chain(items).collect()
        }
        StoredRecord::Event { schema, .. } | StoredRecord::Projection { schema, .. } => {
            return Err(CoreError::EventLog(format!(
                "unsupported event schema {schema} in session {expected_session}"
            )))
        }
        StoredRecord::Header { .. } => {
            return Err(CoreError::EventLog(format!(
                "duplicate header in session {expected_session}"
            )))
        }
    };
    for event in &group {
        if event.session_id != expected_session {
            return Err(CoreError::EventLog(format!(
                "event {} belongs to {} instead of {expected_session}",
                event.id, event.session_id
            )));
        }
        if event.sequence != expected_sequence {
            return Err(CoreError::EventLog(format!(
                "session {expected_session} has sequence {} at position {expected_sequence}",
                event.sequence
            )));
        }
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or_else(|| CoreError::EventLog("session sequence overflow".into()))?;
    }
    Ok((group, projection))
}

fn decode_frames(bytes: &[u8]) -> Result<(Vec<StoredRecord>, usize, usize)> {
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        // A write can stop within the magic itself. Only its exact prefix is
        // a recoverable short tail; arbitrary/corrupt bytes still fail below.
        if remaining.len() < ZSTD_MAGIC.len() && ZSTD_MAGIC.starts_with(remaining) {
            return Ok((records, offset, remaining.len()));
        }
        let frame_size = match zstd::zstd_safe::find_frame_compressed_size(remaining) {
            Ok(size) => size,
            Err(code) if remaining.starts_with(&ZSTD_MAGIC) && is_truncated_frame(code) => {
                return Ok((records, offset, remaining.len()));
            }
            Err(code) => {
                return Err(CoreError::EventLog(format!(
                    "invalid zstd frame at byte {offset}: {}",
                    zstd::zstd_safe::get_error_name(code)
                )))
            }
        };
        let end = offset
            .checked_add(frame_size)
            .ok_or_else(|| CoreError::EventLog("session frame size overflow".to_string()))?;
        if end > bytes.len() {
            return Ok((records, offset, bytes.len() - offset));
        }
        let decoded = zstd::decode_all(&bytes[offset..end]).map_err(|error| {
            CoreError::EventLog(format!("corrupt zstd frame at byte {offset}: {error}"))
        })?;
        for line in decoded.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            records.push(serde_json::from_slice(line).map_err(|error| {
                CoreError::EventLog(format!("invalid JSONL record at byte {offset}: {error}"))
            })?);
        }
        offset = end;
    }
    Ok((records, offset, 0))
}

fn is_truncated_frame(code: usize) -> bool {
    let name = zstd::zstd_safe::get_error_name(code);
    name.contains("Src size is incorrect") || name.contains("srcSize_wrong")
}

fn encode_records(records: &[StoredRecord]) -> Result<Vec<u8>> {
    let mut jsonl = Vec::new();
    for record in records {
        serde_json::to_writer(&mut jsonl, record)?;
        jsonl.push(b'\n');
    }
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 0).map_err(io_error)?;
    encoder.include_checksum(true).map_err(io_error)?;
    encoder.write_all(&jsonl).map_err(io_error)?;
    encoder.finish().map_err(io_error)
}

fn write_generation(
    path: &Path,
    generation: u64,
    header: &SessionFileHeader,
    events: &[Event],
    projections: &[ProjectionSpan],
) -> Result<u64> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::Persistence("session path has no parent".into()))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let temp = path.with_extension(format!("tmp-{}-{}", std::process::id(), now_nanos()));
    let result = (|| -> Result<u64> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(io_error)?;
        let header_record = StoredRecord::Header {
            format: FORMAT_VERSION,
            generation,
            session: header.clone(),
        };
        file.write_all(&encode_records(&[header_record])?)
            .map_err(io_error)?;
        if !events.is_empty() {
            let mut records = Vec::new();
            let mut index = 0;
            for span in projections {
                if span.start < index as u64
                    || span.end < span.start
                    || span.end >= events.len() as u64
                {
                    return Err(CoreError::EventLog(
                        "invalid projection span during generation rewrite".into(),
                    ));
                }
                while index < span.start as usize {
                    records.push(StoredRecord::Event {
                        schema: FORMAT_VERSION,
                        event: events[index].clone(),
                    });
                    index += 1;
                }
                records.push(StoredRecord::Projection {
                    schema: FORMAT_VERSION,
                    source: events[index].clone(),
                    items: events[index + 1..=span.end as usize].to_vec(),
                });
                index = span.end as usize + 1;
            }
            records.extend(
                events[index..]
                    .iter()
                    .cloned()
                    .map(|event| StoredRecord::Event {
                        schema: FORMAT_VERSION,
                        event,
                    }),
            );
            file.write_all(&encode_records(&records)?)
                .map_err(io_error)?;
        }
        file.sync_all().map_err(io_error)?;
        // No fallible inspection after publication: callers need Err to mean
        // the old authoritative generation is still in effect.
        let byte_len = file.metadata().map_err(io_error)?.len();
        fs::rename(&temp, path).map_err(io_error)?;
        Ok(byte_len)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn current_generation(root: &Path, session_id: SessionId) -> Result<Option<(u64, PathBuf)>> {
    let dir = session_dir(root, session_id);
    current_generation_in(&dir)
}

fn current_generation_in(dir: &Path) -> Result<Option<(u64, PathBuf)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let mut selected: Option<(u64, PathBuf)> = None;
    for entry in entries {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(number) = name
            .strip_prefix("session.g")
            .and_then(|rest| rest.strip_suffix(FILE_SUFFIX))
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        if selected
            .as_ref()
            .map_or(true, |(generation, _)| number > *generation)
        {
            selected = Some((number, entry.path()));
        }
    }
    Ok(selected)
}

/// Retention must include restorable recycle-area history, not just the active
/// generation. Caller owns the per-session I/O lock; never repair logs in GC.
pub(crate) fn load_retained_events(root: &Path, session_id: SessionId) -> Result<Vec<Event>> {
    let mut directories = vec![session_dir(root, session_id)];
    match fs::read_dir(trash_session_root(root, session_id)) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(io_error)?;
                if entry.file_type().map_err(io_error)?.is_dir()
                    && entry.file_name().to_string_lossy().parse::<i64>().is_ok()
                {
                    directories.push(entry.path());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    let mut events = Vec::new();
    let mut found = false;
    for directory in directories {
        let Some((generation, path)) = current_generation_in(&directory)? else {
            continue;
        };
        found = true;
        let mut file = open_session_file(&path, false)?;
        let loaded = read_locked_file(&mut file, generation, session_id, false)?;
        if loaded.recovered_tail_bytes != 0 {
            return Err(CoreError::EventLog(format!(
                "incomplete retained history for {session_id}; tool result GC refused"
            )));
        }
        events.extend(loaded.events);
    }
    if !found {
        return Err(CoreError::EventLog(format!(
            "retained session {session_id} has no readable source; tool result GC refused"
        )));
    }
    Ok(events)
}

pub(crate) fn lock_session(root: &Path, session_id: SessionId) -> Result<SessionLock> {
    let file = open_session_lock(root, session_id, "lock")?;
    lock_exclusive_with_timeout(&file)?;
    Ok(SessionLock { file })
}

/// The lifecycle gate is separate from the short event-I/O lock: a running
/// turn must still be able to append while it holds shared lifecycle access.
/// Both locks live outside the replaceable session directory/generations.
pub(crate) fn try_lock_session_control(
    root: &Path,
    session_id: SessionId,
    exclusive: bool,
) -> Result<SessionLock> {
    require_session_scope(root, session_id, "control.lock", exclusive)
}

pub(crate) fn try_lock_session_dispatch(root: &Path, session_id: SessionId) -> Result<SessionLock> {
    require_session_scope(root, session_id, "dispatch.lock", true)
}

fn require_session_scope(
    root: &Path,
    session_id: SessionId,
    scope: &str,
    exclusive: bool,
) -> Result<SessionLock> {
    try_lock_session_scope(root, session_id, scope, exclusive)?.ok_or_else(|| {
        CoreError::invalid(format!(
            "session {session_id} is busy ({scope}); wait for its owner to finish"
        ))
    })
}

/// Contention is an ordinary skipped recovery, not an I/O failure. Never infer
/// contention from an error string or suppress lock-open/permission failures.
pub(crate) fn try_lock_session_scope(
    root: &Path,
    session_id: SessionId,
    scope: &str,
    exclusive: bool,
) -> Result<Option<SessionLock>> {
    let file = open_session_lock(root, session_id, scope)?;
    let result = if exclusive {
        FileExt::try_lock_exclusive(&file)
    } else {
        FileExt::try_lock_shared(&file)
    };
    match result {
        Ok(()) => Ok(Some(SessionLock { file })),
        Err(error)
            if error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
                || error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            tracing::info!(%session_id, scope, exclusive, "session access refused while busy");
            Ok(None)
        }
        Err(error) => Err(CoreError::Persistence(format!(
            "acquire session {session_id} {scope}: {error}"
        ))),
    }
}

fn open_session_lock(root: &Path, session_id: SessionId, suffix: &str) -> Result<File> {
    let lock_root = root.parent().unwrap_or(root).join("locks").join("sessions");
    fs::create_dir_all(&lock_root).map_err(io_error)?;
    let path = lock_root.join(format!("{session_id}.{suffix}"));
    let deadline = Instant::now() + SESSION_LOCK_TIMEOUT;
    let file = loop {
        match OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => break file,
            Err(error) if is_lock_busy(&error) && Instant::now() < deadline => {
                thread::sleep(SESSION_LOCK_POLL);
            }
            Err(error) if is_lock_busy(&error) => {
                return Err(CoreError::Persistence(format!(
                    "timed out opening session lock after {}ms",
                    SESSION_LOCK_TIMEOUT.as_millis()
                )));
            }
            Err(error) => return Err(io_error(error)),
        }
    };
    Ok(file)
}

fn lock_exclusive_with_timeout(file: &File) -> Result<()> {
    let deadline = Instant::now() + SESSION_LOCK_TIMEOUT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) if is_lock_busy(&error) && Instant::now() < deadline => {
                thread::sleep(SESSION_LOCK_POLL);
            }
            Err(error) if is_lock_busy(&error) => {
                return Err(CoreError::Persistence(format!(
                    "timed out waiting for session lock after {}ms",
                    SESSION_LOCK_TIMEOUT.as_millis()
                )));
            }
            Err(error) => return Err(io_error(error)),
        }
    }
}

fn open_session_file(path: &Path, append: bool) -> Result<File> {
    let deadline = Instant::now() + SESSION_LOCK_TIMEOUT;
    loop {
        let mut options = OpenOptions::new();
        options.read(true);
        if append {
            options.append(true);
        } else {
            options.write(true);
        }
        match options.open(path) {
            Ok(file) => return Ok(file),
            Err(error) if is_lock_busy(&error) && Instant::now() < deadline => {
                thread::sleep(SESSION_LOCK_POLL);
            }
            Err(error) if is_lock_busy(&error) => {
                return Err(CoreError::Persistence(format!(
                    "timed out opening session file after {}ms",
                    SESSION_LOCK_TIMEOUT.as_millis()
                )));
            }
            Err(error) => return Err(io_error(error)),
        }
    }
}

fn is_lock_busy(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(33)
        || matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::PermissionDenied
        )
}

pub(crate) fn session_dir(root: &Path, session_id: SessionId) -> PathBuf {
    let id = session_id.to_string();
    let shard: String = id
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .take(2)
        .collect();
    root.join(shard).join(id)
}

fn trash_session_root(root: &Path, session_id: SessionId) -> PathBuf {
    root.parent()
        .unwrap_or(root)
        .join("trash")
        .join("sessions")
        .join(session_id.to_string())
}

fn generation_path(dir: &Path, generation: u64) -> PathBuf {
    dir.join(format!("session.g{generation:06}{FILE_SUFFIX}"))
}

fn io_error(error: std::io::Error) -> CoreError {
    CoreError::Persistence(error.to_string())
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::event::EventPayload;
    use deepagent_core::id::EventId;

    fn header(session_id: SessionId) -> SessionFileHeader {
        SessionFileHeader {
            id: session_id,
            title: Some("file test".into()),
            mode: SessionMode::Normal,
            project: None,
            created_at: Timestamp::from_millis(1),
            execution_config: None,
            plan_mode_version: None,
        }
    }

    #[test]
    fn execution_config_legacy_header_and_unknown_version_are_not_conflated() {
        use deepagent_core::execution_config::{ExecutionOverrides, SessionExecutionConfig};
        let legacy = serde_json::to_value(header(SessionId::new())).unwrap();
        assert!(legacy.get("execution_config").is_none());
        assert!(serde_json::from_value::<SessionFileHeader>(legacy.clone())
            .unwrap()
            .execution_config
            .is_none());
        let mut value = legacy;
        let mut config = SessionExecutionConfig::new(ExecutionOverrides::default());
        config.version = 2;
        value["execution_config"] = serde_json::to_value(config).unwrap();
        let parsed = serde_json::from_value::<SessionFileHeader>(value.clone()).unwrap();
        assert!(parsed.execution_config.unwrap().validate().is_err());
        value["execution_config"]["overrides"]["permission_profile"] = serde_json::json!(42);
        assert!(serde_json::from_value::<SessionFileHeader>(value).is_err());
    }

    fn event(session_id: SessionId, text: &str) -> Event {
        Event {
            id: EventId::new(),
            session_id,
            sequence: 0,
            timestamp: Timestamp::from_millis(2),
            payload: EventPayload::Note { text: text.into() },
        }
    }

    #[test]
    fn projection_frame_is_all_or_none_at_every_incomplete_frame_boundary() {
        use deepagent_core::message::Message;
        use deepagent_core::response_item::ResponseItem;

        let temp = tempfile::tempdir().unwrap();
        let id = SessionId::new();
        create(temp.path(), header(id)).unwrap();
        let (_, path) = current_generation(temp.path(), id).unwrap().unwrap();
        let prefix = fs::read(&path).unwrap();
        let mut source = event(id, "unused");
        let mut message = Message::assistant("whole text");
        message.reasoning_content = Some("whole reasoning".into());
        source.payload = EventPayload::MessageAppended { message };
        let items: Vec<_> = [
            ResponseItem::Reasoning {
                id: None,
                content: "whole reasoning".into(),
            },
            ResponseItem::Message {
                role: "assistant".into(),
                content: "whole text".into(),
            },
        ]
        .into_iter()
        .enumerate()
        .map(|(index, item)| Event {
            id: EventId::new(),
            session_id: id,
            sequence: index as u64 + 1,
            timestamp: source.timestamp,
            payload: EventPayload::ResponseItemAppended { item },
        })
        .collect();
        let frame = encode_records(&[StoredRecord::Projection {
            schema: FORMAT_VERSION,
            source,
            items,
        }])
        .unwrap();
        for cut in 0..frame.len() {
            let mut bytes = prefix.clone();
            bytes.extend_from_slice(&frame[..cut]);
            fs::write(&path, bytes).unwrap();
            let recovered = load(temp.path(), id).unwrap().unwrap();
            assert!(
                recovered.events.is_empty(),
                "partial source/items at frame cut {cut}"
            );
            assert!(recovered.projections.is_empty());
            assert_eq!(fs::metadata(&path).unwrap().len(), prefix.len() as u64);
        }
        let mut bytes = prefix;
        bytes.extend_from_slice(&frame);
        fs::write(&path, &bytes).unwrap();
        let complete = load(temp.path(), id).unwrap().unwrap();
        assert_eq!(complete.events.len(), 3);
        assert_eq!(complete.projections, [ProjectionSpan { start: 0, end: 2 }]);
        let (rewritten, removed) = rewrite_prefix(temp.path(), id, 0).unwrap();
        assert_eq!(removed, 0);
        assert_eq!(rewritten.events, complete.events);
        assert_eq!(rewritten.projections, complete.projections);
        rewrite_header_project(temp.path(), id, "new-project").unwrap();
        let again = load(temp.path(), id).unwrap().unwrap();
        assert_eq!(again.projections, complete.projections);
        assert_eq!(again.events, complete.events);
    }

    #[test]
    fn incomplete_final_frame_is_removed_without_losing_committed_events() {
        let temp = tempfile::tempdir().unwrap();
        let session_id = SessionId::new();
        let created = create(temp.path(), header(session_id)).unwrap();
        let appended = append(
            temp.path(),
            session_id,
            event(session_id, "committed"),
            Some(SessionFileCursor {
                generation: created.generation,
                last_sequence: None,
                byte_len: created.byte_len,
            }),
        )
        .unwrap();
        let (_, path) = current_generation(temp.path(), session_id)
            .unwrap()
            .unwrap();
        let partial = encode_records(&[StoredRecord::Event {
            schema: FORMAT_VERSION,
            event: event(session_id, "partial"),
        }])
        .unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&partial[..partial.len() / 2]).unwrap();
        file.sync_all().unwrap();
        assert!(fs::metadata(&path).unwrap().len() > appended.byte_len);

        let recovered = load(temp.path(), session_id).unwrap().unwrap();
        assert_eq!(recovered.events.len(), 1);
        assert!(recovered.recovered_tail_bytes > 0);
        assert_eq!(fs::metadata(&path).unwrap().len(), appended.byte_len);
    }

    #[test]
    fn checksum_corruption_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let session_id = SessionId::new();
        let created = create(temp.path(), header(session_id)).unwrap();
        append(
            temp.path(),
            session_id,
            event(session_id, "will corrupt"),
            Some(SessionFileCursor {
                generation: created.generation,
                last_sequence: None,
                byte_len: created.byte_len,
            }),
        )
        .unwrap();
        let (_, path) = current_generation(temp.path(), session_id)
            .unwrap()
            .unwrap();
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&path, bytes).unwrap();

        let error = load(temp.path(), session_id).unwrap_err();
        assert!(matches!(error, CoreError::EventLog(_)));
    }

    #[test]
    fn non_contiguous_sequence_is_rejected() {
        let session_id = SessionId::new();
        let mut bad_event = event(session_id, "gap");
        bad_event.sequence = 3;
        let error = parse_records(
            vec![
                StoredRecord::Header {
                    format: FORMAT_VERSION,
                    generation: 1,
                    session: header(session_id),
                },
                StoredRecord::Event {
                    schema: FORMAT_VERSION,
                    event: bad_event,
                },
            ],
            1,
            session_id,
            10,
            0,
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::EventLog(_)));
        assert!(error.to_string().contains("sequence 3 at position 0"));
    }

    #[test]
    fn trash_gc_removes_only_entries_at_or_before_cutoff() {
        let temp = tempfile::tempdir().unwrap();
        let old_id = SessionId::new();
        let fresh_id = SessionId::new();
        create(temp.path(), header(old_id)).unwrap();
        create(temp.path(), header(fresh_id)).unwrap();
        assert!(move_to_trash(temp.path(), old_id, 100).unwrap());
        assert!(move_to_trash(temp.path(), fresh_id, 300).unwrap());

        assert_eq!(purge_trash_before(temp.path(), 200).unwrap(), 1);
        assert!(!restore_from_trash(temp.path(), old_id).unwrap());
        assert!(restore_from_trash(temp.path(), fresh_id).unwrap());
        assert!(load(temp.path(), fresh_id).unwrap().is_some());
    }
}
