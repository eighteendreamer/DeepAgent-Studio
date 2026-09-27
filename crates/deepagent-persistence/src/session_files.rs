use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionFileHeader {
    pub(crate) id: SessionId,
    pub(crate) title: Option<String>,
    pub(crate) mode: SessionMode,
    pub(crate) project: Option<String>,
    pub(crate) created_at: Timestamp,
}

#[derive(Debug, Serialize, Deserialize)]
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
}

#[derive(Debug)]
pub(crate) struct LoadedSessionFile {
    pub(crate) generation: u64,
    pub(crate) header: SessionFileHeader,
    pub(crate) events: Vec<Event>,
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
    pub(crate) generation: u64,
    pub(crate) byte_len: u64,
    pub(crate) recovered_tail_bytes: u64,
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

    write_generation(&path, generation, &header, &[])?;
    let byte_len = fs::metadata(&path).map_err(io_error)?.len();
    Ok(LoadedSessionFile {
        generation,
        header,
        events: Vec::new(),
        byte_len,
        recovered_tail_bytes: 0,
    })
}

pub(crate) fn load(root: &Path, session_id: SessionId) -> Result<Option<LoadedSessionFile>> {
    let Some((generation, path)) = current_generation(root, session_id)? else {
        return Ok(None);
    };
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(io_error)?;
    file.lock_exclusive().map_err(io_error)?;
    let result = read_locked_file(&mut file, generation, session_id, true);
    let unlock_result = FileExt::unlock(&file).map_err(io_error);
    match (result, unlock_result) {
        (Ok(loaded), Ok(())) => Ok(Some(loaded)),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
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

pub(crate) fn append(
    root: &Path,
    session_id: SessionId,
    mut event: Event,
    cursor: Option<SessionFileCursor>,
) -> Result<AppendedSessionEvent> {
    let (generation, path) = current_generation(root, session_id)?.ok_or_else(|| {
        CoreError::EventLog(format!("session {session_id} has no persisted event file"))
    })?;
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .open(&path)
        .map_err(io_error)?;
    file.lock_exclusive().map_err(io_error)?;

    let file_len = file.metadata().map_err(io_error)?.len();
    let matching_cursor =
        cursor.filter(|cursor| cursor.generation == generation && cursor.byte_len == file_len);
    let recovered_tail_bytes;
    event.sequence = if let Some(cursor) = matching_cursor {
        recovered_tail_bytes = 0;
        cursor.last_sequence.map_or(0, |sequence| sequence + 1)
    } else {
        let loaded = read_locked_file(&mut file, generation, session_id, true)?;
        recovered_tail_bytes = loaded.recovered_tail_bytes;
        loaded.events.len() as u64
    };

    let record = StoredRecord::Event {
        schema: FORMAT_VERSION,
        event: event.clone(),
    };
    let encoded = encode_records(std::slice::from_ref(&record))?;
    file.seek(SeekFrom::End(0)).map_err(io_error)?;
    file.write_all(&encoded).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    let byte_len = file.metadata().map_err(io_error)?.len();
    FileExt::unlock(&file).map_err(io_error)?;
    Ok(AppendedSessionEvent {
        event,
        generation,
        byte_len,
        recovered_tail_bytes,
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
    let kept: Vec<Event> = current
        .events
        .iter()
        .filter(|event| event.sequence <= keep_through)
        .cloned()
        .collect();
    let removed = current.events.len().saturating_sub(kept.len()) as u64;
    let generation = current.generation + 1;
    let dir = session_dir(root, session_id);
    let path = generation_path(&dir, generation);
    write_generation(&path, generation, &current.header, &kept)?;
    let byte_len = fs::metadata(&path).map_err(io_error)?.len();
    Ok((
        LoadedSessionFile {
            generation,
            header: current.header,
            events: kept,
            byte_len,
            recovered_tail_bytes: 0,
        },
        removed,
    ))
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
    let header = match records.next() {
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

    let mut events = Vec::new();
    for record in records {
        match record {
            StoredRecord::Event { schema, event } if schema == FORMAT_VERSION => {
                if event.session_id != expected_session {
                    return Err(CoreError::EventLog(format!(
                        "event {} belongs to {} instead of {expected_session}",
                        event.id, event.session_id
                    )));
                }
                let expected_sequence = events.len() as u64;
                if event.sequence != expected_sequence {
                    return Err(CoreError::EventLog(format!(
                        "session {expected_session} has sequence {} at position {expected_sequence}",
                        event.sequence
                    )));
                }
                events.push(event);
            }
            StoredRecord::Event { schema, .. } => {
                return Err(CoreError::EventLog(format!(
                    "unsupported event schema {schema} in session {expected_session}"
                )))
            }
            StoredRecord::Header { .. } => {
                return Err(CoreError::EventLog(format!(
                    "duplicate header in session {expected_session}"
                )))
            }
        }
    }

    Ok(LoadedSessionFile {
        generation,
        header,
        events,
        byte_len,
        recovered_tail_bytes,
    })
}

fn decode_frames(bytes: &[u8]) -> Result<(Vec<StoredRecord>, usize, usize)> {
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
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
) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::Persistence("session path has no parent".into()))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let temp = path.with_extension(format!("tmp-{}-{}", std::process::id(), now_nanos()));
    let result = (|| -> Result<()> {
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
            let records: Vec<StoredRecord> = events
                .iter()
                .cloned()
                .map(|event| StoredRecord::Event {
                    schema: FORMAT_VERSION,
                    event,
                })
                .collect();
            file.write_all(&encode_records(&records)?)
                .map_err(io_error)?;
        }
        file.sync_all().map_err(io_error)?;
        fs::rename(&temp, path).map_err(io_error)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn current_generation(root: &Path, session_id: SessionId) -> Result<Option<(u64, PathBuf)>> {
    let dir = session_dir(root, session_id);
    let entries = match fs::read_dir(&dir) {
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

pub(crate) fn lock_session(root: &Path, session_id: SessionId) -> Result<SessionLock> {
    let lock_root = root.parent().unwrap_or(root).join("locks").join("sessions");
    fs::create_dir_all(&lock_root).map_err(io_error)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_root.join(format!("{session_id}.lock")))
        .map_err(io_error)?;
    file.lock_exclusive().map_err(io_error)?;
    Ok(SessionLock { file })
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
        }
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
