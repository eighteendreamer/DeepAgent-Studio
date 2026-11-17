//! Bounded reads of the authoritative compressed file, not the SQLite index.
//! A continuation is a position inside a fixed generation/byte-length cut.
//! The transport must integrity-protect it; it is not an authorization grant.

use super::*;
use std::collections::VecDeque;
use std::io::BufReader;

const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;
const MAX_WINDOW_BYTES: usize = 8 * 1024 * 1024;
const MAX_WINDOW_RECORDS: usize = crate::bounded_read::MAX_HISTORY_ITEMS + 1;
const MAX_SCAN_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SCAN_FRAMES: usize = 256;
const READ_CHUNK: usize = 4096;

/// Read only the first bounded, CRC-checked authority frame. No transcript
/// materialization or provider inference is needed for metadata filtering.
pub(crate) fn catalog_header(root: &Path, session: SessionId) -> Result<Option<SessionFileHeader>> {
    let Some((generation, path)) = current_generation(root, session)? else {
        return Ok(None);
    };
    let mut file = File::open(path).map_err(io_error)?;
    let size = file.metadata().map_err(io_error)?.len();
    let frame = read_frame_bounded(&mut file, 0, size.min(1024 * 1024), 0, Some(1024 * 1024))?;
    let header = parse_header(frame.records.into_iter().next(), generation, session)?;
    if let Some(config) = &header.execution_config {
        config.validate()?;
    }
    Ok(Some(header))
}

/// Storage continuation. Fields are private so ordinary callers cannot invent
/// positions; deserialize only after the transport validates its envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionPageCursor {
    session_id: SessionId,
    generation: u64,
    through_bytes: u64,
    frame_offset: u64,
    record_index: usize,
    next_sequence: u64,
    after_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct SessionEventPage {
    pub events: Vec<Event>,
    /// End indices and storage checkpoints for indivisible commit groups. A
    /// transport can stop before any group after counting its own envelope.
    pub groups: Vec<(usize, SessionPageCursor)>,
    pub start_cursor: SessionPageCursor,
    /// Includes the exhausted checkpoint so later projection pages can still
    /// validate the original generation without rereading session history.
    pub end_cursor: SessionPageCursor,
    pub next_cursor: Option<SessionPageCursor>,
    pub generation: u64,
    pub through_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionPageBudget {
    pub max_events: usize,
    /// Event JSON bytes, excluding the transport envelope and stream metadata.
    pub max_bytes: usize,
}

pub(crate) fn read(
    root: &Path,
    session_id: SessionId,
    after_sequence: Option<u64>,
    cursor: Option<SessionPageCursor>,
    budget: SessionPageBudget,
) -> Result<SessionEventPage> {
    if !(1..=crate::bounded_read::MAX_HISTORY_ITEMS).contains(&budget.max_events)
        || !(1..=crate::bounded_read::MAX_HISTORY_BYTES).contains(&budget.max_bytes)
    {
        return Err(CoreError::invalid("invalid session history page budget"));
    }
    let (generation, path) = current_generation(root, session_id)?
        .ok_or_else(|| CoreError::not_found(format!("session {session_id}")))?;
    let mut file = open_session_file(&path, false)?;
    lock_exclusive_with_timeout(&file)?;
    let result = read_locked(
        &mut file,
        session_id,
        generation,
        after_sequence,
        cursor,
        budget,
    );
    let unlock = FileExt::unlock(&file).map_err(io_error);
    match (result, unlock) {
        (Ok(page), Ok(())) => Ok(page),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

fn read_locked(
    file: &mut File,
    session_id: SessionId,
    generation: u64,
    after_sequence: Option<u64>,
    cursor: Option<SessionPageCursor>,
    budget: SessionPageBudget,
) -> Result<SessionEventPage> {
    let file_bytes = file.metadata().map_err(io_error)?.len();
    let position = match cursor {
        Some(cursor)
            if cursor.session_id == session_id
                && cursor.generation == generation
                && cursor.through_bytes <= file_bytes
                && cursor.frame_offset <= cursor.through_bytes
                && cursor.after_sequence == after_sequence =>
        {
            cursor
        }
        Some(_) => {
            return Err(CoreError::invalid(
                "stale or foreign session history cursor",
            ))
        }
        None => SessionPageCursor {
            session_id,
            generation,
            through_bytes: file_bytes,
            frame_offset: 0,
            record_index: 1,
            next_sequence: 0,
            after_sequence,
        },
    };
    let start_cursor = position.clone();
    let mut reader = FileGroups::new(file, position)?;
    let mut events = Vec::new();
    let mut groups = Vec::new();
    let mut bytes = 2;
    while let Some(group) = reader.next_group(budget)? {
        // Preserve an explicit legacy starting sequence filter. Only our own
        // continuation cuts, not that caller-provided filter, are group-aligned.
        let selected: Vec<_> = group
            .events
            .into_iter()
            .filter(|event| after_sequence.is_none_or(|after| event.sequence > after))
            .collect();
        let group_bytes = crate::bounded_read::json_len(&selected, budget.max_bytes)?;
        if selected.len() > budget.max_events {
            return Err(group_too_large());
        }
        let next_bytes = if selected.is_empty() {
            bytes
        } else {
            bytes + group_bytes - 2 + usize::from(!events.is_empty())
        };
        if !events.is_empty()
            && (events.len() + selected.len() > budget.max_events || next_bytes > budget.max_bytes)
        {
            return Ok(finish(events, groups, start_cursor, group.before));
        }
        bytes = next_bytes;
        events.extend(selected);
        groups.push((events.len(), reader.position().clone()));
        if events.len() == budget.max_events {
            break;
        }
    }
    Ok(finish(
        events,
        groups,
        start_cursor,
        reader.position().clone(),
    ))
}

struct EventGroup {
    events: Vec<Event>,
    legacy: bool,
    before: SessionPageCursor,
}

/// One bounded, checksummed record stream with a one-group lookahead. Legacy
/// Event records have no explicit ownership marker, so only adjacent items
/// accepted by the shared event-projection rule are grouped.
struct FileGroups<'a> {
    file: &'a mut File,
    position: SessionPageCursor,
    records: VecDeque<StoredRecord>,
    frame_bytes: u64,
    frame_complete: bool,
    scanned: u64,
    frames: usize,
    pending: Option<EventGroup>,
}

impl<'a> FileGroups<'a> {
    fn new(file: &'a mut File, position: SessionPageCursor) -> Result<Self> {
        let header = read_frame(file, 0, position.through_bytes, 0)?;
        parse_header(
            header.records.first().cloned(),
            position.generation,
            position.session_id,
        )?;
        let mut reader = Self {
            file,
            position,
            records: VecDeque::new(),
            frame_bytes: 0,
            frame_complete: false,
            scanned: header.compressed_bytes + header.decoded_bytes,
            frames: 1,
            pending: None,
        };
        if reader.position.frame_offset == 0 {
            reader.install(header)?;
        }
        Ok(reader)
    }

    fn position(&self) -> &SessionPageCursor {
        self.pending
            .as_ref()
            .map_or(&self.position, |group| &group.before)
    }

    fn install(&mut self, frame: DecodedFrame) -> Result<()> {
        if self.position.record_index < frame.start_index
            || self.position.record_index > frame.total_records
        {
            return Err(CoreError::invalid("invalid session history record cursor"));
        }
        self.frame_bytes = frame.compressed_bytes;
        self.frame_complete = frame.start_index + frame.records.len() == frame.total_records;
        self.records = frame
            .records
            .into_iter()
            .skip(self.position.record_index - frame.start_index)
            .collect();
        if self.records.is_empty() && self.frame_complete {
            self.advance_frame();
        }
        Ok(())
    }

    fn advance_frame(&mut self) {
        self.position.frame_offset += self.frame_bytes;
        self.position.record_index = 0;
        self.frame_bytes = 0;
    }

    fn next_record(&mut self) -> Result<Option<EventGroup>> {
        if let Some(group) = self.pending.take() {
            return Ok(Some(group));
        }
        while self.records.is_empty() {
            if self.position.frame_offset == self.position.through_bytes
                || self.frames >= MAX_SCAN_FRAMES
                || self.scanned >= MAX_SCAN_BYTES
            {
                return Ok(None);
            }
            let frame = read_frame(
                self.file,
                self.position.frame_offset,
                self.position.through_bytes,
                self.position.record_index,
            )?;
            self.scanned += frame.compressed_bytes + frame.decoded_bytes;
            self.frames += 1;
            self.install(frame)?;
        }
        let before = self.position.clone();
        let record = self
            .records
            .pop_front()
            .ok_or_else(|| CoreError::EventLog("empty history record buffer".into()))?;
        let legacy = matches!(record, StoredRecord::Event { .. });
        let (events, _) = parse_event_group(
            record,
            self.position.session_id,
            self.position.next_sequence,
        )?;
        self.position.next_sequence = self
            .position
            .next_sequence
            .checked_add(events.len() as u64)
            .ok_or_else(|| CoreError::EventLog("session history sequence overflow".into()))?;
        self.position.record_index += 1;
        if self.records.is_empty() && self.frame_complete {
            self.advance_frame();
        }
        Ok(Some(EventGroup {
            events,
            legacy,
            before,
        }))
    }

    fn next_group(&mut self, budget: SessionPageBudget) -> Result<Option<EventGroup>> {
        let Some(mut group) = self.next_record()? else {
            return Ok(None);
        };
        if !group.legacy {
            return Ok(Some(group));
        }
        let Some(source) = group.events.first() else {
            return Err(CoreError::EventLog("empty legacy event group".into()));
        };
        if matches!(
            source.payload,
            deepagent_core::event::EventPayload::ResponseItemAppended { .. }
        ) {
            return self.native_first_group(group, budget).map(Some);
        }
        let source_payload = source.payload.clone();
        let expected = crate::event_projection::response_items_for_payload(&[], &source_payload)?;
        if expected.is_empty() {
            return Ok(Some(group));
        }
        let mut bytes = crate::bounded_read::json_len(&group.events, budget.max_bytes)?;
        let mut matched = 0;
        while matched < expected.len() {
            let Some(next) = self.next_record()? else {
                if self.position.frame_offset != self.position.through_bytes {
                    return Err(CoreError::invalid(
                        "history_projection_lookahead_limit: cannot confirm legacy group within scan budget",
                    ));
                }
                break;
            };
            let is_match = next.legacy
                && next.events.first().is_some_and(|event| {
                    crate::event_projection::legacy_projection_matches(
                        &source_payload,
                        &expected[matched],
                        event,
                    )
                });
            if !is_match {
                self.pending = Some(next);
                break;
            }
            if group.events.len() + next.events.len() > budget.max_events {
                return Err(group_too_large());
            }
            bytes += crate::bounded_read::json_len(&next.events, budget.max_bytes)? - 1;
            if bytes > budget.max_bytes {
                return Err(group_too_large());
            }
            group.events.extend(next.events);
            matched += 1;
        }

        if matched > 0 && matched < expected.len() {
            return Err(CoreError::invalid(
                "incomplete legacy source/native projection group",
            ));
        }
        crate::bounded_read::json_len(&group.events, budget.max_bytes)?;
        Ok(Some(group))
    }

    fn native_first_group(
        &mut self,
        mut group: EventGroup,
        budget: SessionPageBudget,
    ) -> Result<EventGroup> {
        let mut bytes = crate::bounded_read::json_len(&group.events, budget.max_bytes)?;
        loop {
            let Some(next) = self.next_record()? else {
                if self.position.frame_offset != self.position.through_bytes {
                    return Err(CoreError::invalid(
                        "history_projection_lookahead_limit: cannot confirm legacy group within scan budget",
                    ));
                }
                break;
            };
            if !next.legacy {
                self.pending = Some(next);
                break;
            }
            let message = matches!(
                next.events[0].payload,
                deepagent_core::event::EventPayload::MessageAppended { .. }
            );
            if message
                && !crate::event_projection::has_native_source(
                    &group.events,
                    &next.events[0].payload,
                    &std::collections::HashSet::new(),
                )?
            {
                self.pending = Some(next);
                break;
            }
            bytes += crate::bounded_read::json_len(&next.events, budget.max_bytes)? - 1;
            if group.events.len() + next.events.len() > budget.max_events
                || bytes > budget.max_bytes
            {
                return Err(group_too_large());
            }
            group.events.extend(next.events);
            if message {
                break;
            }
        }
        Ok(group)
    }
}

fn group_too_large() -> CoreError {
    CoreError::invalid("history_item_too_large: session commit group exceeds page budget")
}

fn finish(
    events: Vec<Event>,
    groups: Vec<(usize, SessionPageCursor)>,
    start_cursor: SessionPageCursor,
    position: SessionPageCursor,
) -> SessionEventPage {
    SessionEventPage {
        events,
        groups,
        start_cursor,
        end_cursor: position.clone(),
        generation: position.generation,
        through_bytes: position.through_bytes,
        next_cursor: (position.frame_offset < position.through_bytes).then_some(position),
    }
}

/// A fixed-size record window from a completely checksummed frame. Legacy
/// rewrites can store arbitrarily many small records in one frame; decode the
/// frame incrementally, retaining only this window. Continuations rescan that
/// old frame (bounded memory, O(frame size) work), never cache a second history.
struct DecodedFrame {
    records: Vec<StoredRecord>,
    start_index: usize,
    total_records: usize,
    compressed_bytes: u64,
    decoded_bytes: u64,
}

#[derive(Default)]
struct RecordWindow {
    records: Vec<StoredRecord>,
    line: Vec<u8>,
    line_bytes: u64,
    retained_bytes: usize,
    index: usize,
    skip: usize,
    full: bool,
}

impl RecordWindow {
    fn consume(&mut self, bytes: &[u8], offset: u64) -> Result<()> {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            let newline = part.last() == Some(&b'\n');
            let content = if newline {
                &part[..part.len() - 1]
            } else {
                part
            };
            self.line_bytes = self
                .line_bytes
                .checked_add(content.len() as u64)
                .ok_or_else(|| CoreError::EventLog("history record size overflow".into()))?;
            if self.index >= self.skip && !self.full {
                if self.line_bytes > MAX_RECORD_BYTES as u64 {
                    return Err(CoreError::invalid(
                        "history_record_too_large: session history record exceeds bounded decoding budget",
                    ));
                }
                self.line.extend_from_slice(content);
            }
            if newline {
                self.finish_line(offset)?;
            }
        }
        Ok(())
    }

    fn finish_line(&mut self, offset: u64) -> Result<()> {
        if self.line_bytes == 0 {
            return Ok(());
        }
        if self.index >= self.skip && !self.full {
            if self.records.len() == MAX_WINDOW_RECORDS
                || (!self.records.is_empty()
                    && self.retained_bytes + self.line.len() > MAX_WINDOW_BYTES)
            {
                self.full = true;
            } else {
                let record = serde_json::from_slice(&self.line).map_err(|_| {
                    CoreError::EventLog(format!("invalid session history JSONL at byte {offset}"))
                })?;
                self.retained_bytes += self.line.len();
                self.records.push(record);
            }
        }
        self.index = self
            .index
            .checked_add(1)
            .ok_or_else(|| CoreError::EventLog("history record count overflow".into()))?;
        self.line.clear();
        self.line_bytes = 0;
        Ok(())
    }
}

fn read_frame(file: &mut File, offset: u64, through: u64, skip: usize) -> Result<DecodedFrame> {
    read_frame_bounded(file, offset, through, skip, None)
}

fn read_frame_bounded(
    file: &mut File,
    offset: u64,
    through: u64,
    skip: usize,
    decoded_limit: Option<u64>,
) -> Result<DecodedFrame> {
    let available = through
        .checked_sub(offset)
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| CoreError::invalid("invalid session frame cursor"))?;
    file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
    let input = BufReader::with_capacity(READ_CHUNK, Read::take(file, available));
    let mut decoder = zstd::stream::read::Decoder::with_buffer(input)
        .map_err(io_error)?
        .single_frame();
    decoder.window_log_max(23).map_err(io_error)?;
    let mut window = RecordWindow {
        skip,
        ..RecordWindow::default()
    };
    let mut chunk = [0; READ_CHUNK];
    let mut decoded_bytes = 0u64;
    loop {
        let read = decoder.read(&mut chunk).map_err(|error| {
            CoreError::EventLog(format!(
                "corrupt session history frame at byte {offset}: {error}"
            ))
        })?;
        if read == 0 {
            break;
        }
        decoded_bytes = decoded_bytes
            .checked_add(read as u64)
            .ok_or_else(|| CoreError::EventLog("history frame size overflow".into()))?;
        if decoded_limit.is_some_and(|limit| decoded_bytes > limit) {
            return Err(CoreError::invalid(
                "catalog_header_too_large: provider metadata frame exceeds 1MiB",
            ));
        }
        window.consume(&chunk[..read], offset)?;
    }
    window.finish_line(offset)?;
    if skip > window.index {
        return Err(CoreError::invalid("invalid session history record cursor"));
    }
    // single_frame leaves any following compressed frame in BufReader. Count
    // only consumed bytes, not read-ahead, so next frame offsets stay exact.
    let input = decoder.finish();
    let compressed_bytes = available - input.get_ref().limit() - input.buffer().len() as u64;
    if compressed_bytes == 0 {
        return Err(CoreError::EventLog("empty compressed history frame".into()));
    }
    Ok(DecodedFrame {
        records: window.records,
        start_index: skip,
        total_records: window.index,
        compressed_bytes,
        decoded_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::EventStore;
    use crate::Database;
    use deepagent_core::event::EventPayload;
    use deepagent_core::message::Message;

    fn fixture(messages: usize) -> (Database, SessionId) {
        let db = Database::open_in_memory().unwrap();
        let id = SessionId::new();
        let store = EventStore::new(&db);
        store
            .create_session(id, None, Timestamp::from_millis(1))
            .unwrap();
        for index in 0..messages {
            append(&db, id, &format!("message-{index}-{}", "x".repeat(128)));
        }
        (db, id)
    }

    fn append(db: &Database, id: SessionId, text: &str) {
        EventStore::new(db)
            .append_with_response_projection(
                id,
                EventPayload::MessageAppended {
                    message: Message::user(text),
                },
                true,
                Timestamp::from_millis(2),
            )
            .unwrap();
    }

    fn budget(max_events: usize) -> SessionPageBudget {
        SessionPageBudget {
            max_events,
            max_bytes: 16 * 1024,
        }
    }

    #[test]
    fn h218_file_page_roundtrip_frozen_append_and_repeat_are_gapless_and_group_aligned() {
        let (db, id) = fixture(21);
        let store = EventStore::new(&db);
        let expected = store.load_session(id).unwrap();
        let first = store.read_session_page(id, None, None, budget(5)).unwrap();
        assert_eq!(first.events.len(), 4);
        let cursor = first.next_cursor.clone().unwrap();
        let second = store
            .read_session_page(id, None, Some(cursor.clone()), budget(5))
            .unwrap();
        let repeated = store
            .read_session_page(id, None, Some(cursor), budget(5))
            .unwrap();
        assert_eq!(second.events, repeated.events);
        assert_eq!(second.next_cursor, repeated.next_cursor);
        append(&db, id, "not-in-the-frozen-cut");
        let mut actual = first.events;
        let mut next = first.next_cursor;
        for _ in 0..30 {
            let Some(cursor) = next else {
                break;
            };
            let page = store
                .read_session_page(id, None, Some(cursor), budget(5))
                .unwrap();
            assert!(page.events.len() <= 5);
            assert!(page.events.len() % 2 == 0);
            assert!(serde_json::to_vec(&page.events).unwrap().len() <= 16 * 1024);
            actual.extend(page.events);
            next = page.next_cursor;
        }
        assert!(next.is_none());
        assert_eq!(actual, expected);
    }

    #[test]
    fn h218_empty_file_history_and_exact_group_budget_finish_without_spurious_cursor() {
        let (db, id) = fixture(0);
        let store = EventStore::new(&db);
        let empty = store.read_session_page(id, None, None, budget(2)).unwrap();
        assert!(empty.events.is_empty());
        assert!(empty.next_cursor.is_none());
        append(&db, id, "exact group");
        let exact = store.read_session_page(id, None, None, budget(2)).unwrap();
        assert_eq!(exact.events.len(), 2);
        assert!(exact.next_cursor.is_none());
    }

    #[test]
    fn h218_cursor_binds_session_generation_and_legacy_filter() {
        let (db, id) = fixture(8);
        let store = EventStore::new(&db);
        let first = store.read_session_page(id, None, None, budget(4)).unwrap();
        let cursor = first.next_cursor.unwrap();
        assert!(store
            .read_session_page(id, None, Some(cursor.clone()), budget(4))
            .is_ok());
        let foreign = SessionId::new();
        store
            .create_session(foreign, None, Timestamp::from_millis(1))
            .unwrap();
        assert!(store
            .read_session_page(foreign, None, Some(cursor.clone()), budget(4))
            .is_err());
        assert!(store
            .read_session_page(id, Some(1), Some(cursor.clone()), budget(4))
            .is_err());
        store.truncate_after(id, 3).unwrap();
        let stale = store
            .read_session_page(id, None, Some(cursor), budget(4))
            .unwrap_err();
        assert!(stale.to_string().contains("stale"));
    }

    #[test]
    fn h218_large_old_rewritten_frame_with_small_records_can_be_paged_read_only() {
        let (db, id) = fixture(0);
        let root = db.session_files_root();
        let current = load(&root, id).unwrap().unwrap();
        let events: Vec<_> = (0..2100)
            .map(|index| Event {
                id: deepagent_core::id::EventId::new(),
                session_id: id,
                sequence: index,
                timestamp: Timestamp::from_millis(2),
                payload: EventPayload::Note {
                    text: format!("old-rewrite-{index}-{}", "x".repeat(4096)),
                },
            })
            .collect();
        // This is the production writer used by historical rewind/fork. Its
        // single frame is >8 MiB decoded, but every individual record is small.
        let path = generation_path(&session_dir(&root, id), current.generation + 1);
        write_generation(&path, current.generation + 1, &current.header, &events, &[]).unwrap();
        let before = fs::read(&path).unwrap();
        let header_frame =
            read_frame(&mut File::open(&path).unwrap(), 0, before.len() as u64, 0).unwrap();
        let window = read_frame(
            &mut File::open(&path).unwrap(),
            header_frame.compressed_bytes,
            before.len() as u64,
            1000,
        )
        .unwrap();
        assert!(window.decoded_bytes > MAX_RECORD_BYTES as u64);
        assert_eq!(window.records.len(), MAX_WINDOW_RECORDS);
        assert_eq!(window.total_records, events.len());
        assert_eq!(window.start_index, 1000);
        let mut cursor = None;
        let mut actual = Vec::new();
        for _ in 0..20 {
            let page = EventStore::new(&db)
                .read_session_page(
                    id,
                    None,
                    cursor,
                    SessionPageBudget {
                        max_events: 500,
                        max_bytes: 1024 * 1024,
                    },
                )
                .unwrap();
            assert!(page.events.len() <= 500);
            assert!(serde_json::to_vec(&page.events).unwrap().len() <= 1024 * 1024);
            actual.extend(page.events);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert!(cursor.is_none());
        assert_eq!(actual, events);
        assert_eq!(fs::read(&path).unwrap(), before);
        // Even corruption after the selected window must prevent delivery.
        let mut corrupted = before.clone();
        *corrupted.last_mut().unwrap() ^= 0x01;
        fs::write(&path, &corrupted).unwrap();
        let error = EventStore::new(&db)
            .read_session_page(id, None, None, budget(3))
            .unwrap_err();
        assert!(error.to_string().contains("corrupt"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), corrupted);
        fs::write(path, before).unwrap();
    }

    #[test]
    fn h218_frame_windows_preserve_unterminated_records_and_exact_frame_offsets() {
        let (_db, id) = fixture(0);
        let records: Vec<_> = (0..3)
            .map(|sequence| StoredRecord::Event {
                schema: FORMAT_VERSION,
                event: Event {
                    id: deepagent_core::id::EventId::new(),
                    session_id: id,
                    sequence,
                    timestamp: Timestamp::from_millis(2),
                    payload: EventPayload::Note { text: "row".into() },
                },
            })
            .collect();
        let mut raw = vec![b'\n'];
        serde_json::to_writer(&mut raw, &records[0]).unwrap();
        raw.extend_from_slice(b"\n\n");
        serde_json::to_writer(&mut raw, &records[1]).unwrap();
        // A valid final JSONL record need not have a trailing newline. Empty
        // lines do not consume record indices, just as in the replay decoder.
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 0).unwrap();
        encoder.include_checksum(true).unwrap();
        encoder.write_all(&raw).unwrap();
        let first = encoder.finish().unwrap();
        let second = encode_records(&records[2..]).unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&first).unwrap();
        file.write_all(&second).unwrap();
        let through = (first.len() + second.len()) as u64;
        let window = read_frame(&mut file, 0, through, 1).unwrap();
        assert_eq!(window.compressed_bytes, first.len() as u64);
        assert_eq!(window.total_records, 2);
        assert_eq!(window.records.len(), 1);
        assert!(
            matches!(&window.records[0], StoredRecord::Event { event, .. } if event.sequence == 1)
        );
        assert!(read_frame(&mut file, 0, through, 2)
            .unwrap()
            .records
            .is_empty());
        assert!(read_frame(&mut file, 0, through, 3).is_err());
        let next = read_frame(&mut file, first.len() as u64, through, 0).unwrap();
        assert_eq!(next.compressed_bytes, second.len() as u64);
        assert_eq!(next.total_records, 1);
        assert!(
            matches!(&next.records[0], StoredRecord::Event { event, .. } if event.sequence == 2)
        );
    }

    #[test]
    fn h218_legacy_projection_group_crosses_record_window_in_both_orders() {
        for native_first in [false, true] {
            let (db, id) = fixture(1);
            let root = db.session_files_root();
            let current = load(&root, id).unwrap().unwrap();
            let mut events: Vec<_> = (0..500)
                .map(|sequence| Event {
                    id: deepagent_core::id::EventId::new(),
                    session_id: id,
                    sequence,
                    timestamp: Timestamp::from_millis(2),
                    payload: EventPayload::Note {
                        text: "prefix".into(),
                    },
                })
                .collect();
            let mut pair = EventStore::new(&db).load_session(id).unwrap();
            if native_first {
                pair.reverse();
            }
            for mut event in pair {
                event.sequence = events.len() as u64;
                events.push(event);
            }
            let path = generation_path(&session_dir(&root, id), current.generation + 1);
            write_generation(&path, current.generation + 1, &current.header, &events, &[]).unwrap();
            let before = fs::read(&path).unwrap();
            let store = EventStore::new(&db);
            assert!(store
                .read_session_page(id, Some(499), None, budget(1))
                .is_err());
            let page = store
                .read_session_page(id, Some(499), None, budget(3))
                .unwrap();
            assert_eq!(page.events, events[500..]);
            assert!(page.next_cursor.is_none());
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn h218_rewritten_multirecord_frame_pages_match_shared_replay_validator() {
        let (db, id) = fixture(12);
        // Rewind republishes one frame containing multiple complete groups.
        EventStore::new(&db).truncate_after(id, 19).unwrap();
        let store = EventStore::new(&db);
        let expected = store.load_session(id).unwrap();
        let mut actual = Vec::new();
        let mut cursor = None;
        for _ in 0..15 {
            let page = store
                .read_session_page(id, None, cursor, budget(4))
                .unwrap();
            actual.extend(page.events);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert!(cursor.is_none());
        assert_eq!(actual, expected);
    }

    #[test]
    fn h218_oversized_commit_group_is_rejected_without_mutating_file() {
        let (db, id) = fixture(0);
        append(&db, id, &"x".repeat(100_000));
        let root = db.session_files_root();
        let (_, path) = current_generation(&root, id).unwrap().unwrap();
        let before = fs::read(&path).unwrap();
        let error = EventStore::new(&db)
            .read_session_page(id, None, None, budget(2))
            .unwrap_err();
        assert!(error.to_string().contains("history_item_too_large"));
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn h218_record_budget_rejects_high_ratio_payload_before_json() {
        let (db, id) = fixture(0);
        append(&db, id, &"x".repeat(MAX_RECORD_BYTES));
        let error = EventStore::new(&db)
            .read_session_page(id, None, None, budget(2))
            .unwrap_err();
        assert!(
            error.to_string().contains("history_record_too_large"),
            "{error}"
        );
    }

    #[test]
    fn h218_corrupt_checksum_and_torn_tail_are_not_returned_or_repaired() {
        let (db, id) = fixture(1);
        let (_, path) = current_generation(&db.session_files_root(), id)
            .unwrap()
            .unwrap();
        let original = fs::read(&path).unwrap();
        for truncate in [false, true] {
            let mut bytes = original.clone();
            if truncate {
                bytes.pop();
            } else {
                *bytes.last_mut().unwrap() ^= 0xff;
            }
            fs::write(&path, &bytes).unwrap();
            assert!(EventStore::new(&db)
                .read_session_page(id, None, None, budget(5))
                .is_err());
            assert_eq!(
                fs::read(&path).unwrap(),
                bytes,
                "page read must not repair historical data"
            );
        }
    }

    #[test]
    fn h218_skip_filter_has_bounded_work_and_empty_pages_make_progress() {
        let (db, id) = fixture(300);
        let store = EventStore::new(&db);
        let first = store
            .read_session_page(id, Some(u64::MAX), None, budget(2))
            .unwrap();
        assert!(first.events.is_empty());
        let cursor = first.next_cursor.unwrap();
        assert!(cursor.frame_offset > 0);
        let second = store
            .read_session_page(id, Some(u64::MAX), Some(cursor), budget(2))
            .unwrap();
        assert!(second.events.is_empty());
        assert!(second.next_cursor.is_none());
    }

    #[test]
    fn h218_legacy_raw_source_native_page_never_returns_a_partial_group() {
        use deepagent_core::response_item::ResponseItem;
        let (db, id) = fixture(0);
        let store = EventStore::new(&db);
        store
            .append(
                id,
                EventPayload::MessageAppended {
                    message: Message::user("legacy source"),
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        store
            .append(
                id,
                EventPayload::ResponseItemAppended {
                    origin: None,
                    item: ResponseItem::Message {
                        role: "user".into(),
                        content: "legacy source".into(),
                    },
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        let single = store.read_session_page(id, None, None, budget(1));
        assert!(
            single.is_err(),
            "legacy source/native pair was split: {single:?}"
        );
        store
            .append(
                id,
                EventPayload::ToolCallCompleted {
                    content: Vec::new(),
                    call_id: "custom-call".into(),
                    ok: true,
                    output: serde_json::json!({"result":"synthetic"}),
                    duration_ms: 1,
                },
                Timestamp::from_millis(3),
            )
            .unwrap();
        store
            .append(
                id,
                EventPayload::ResponseItemAppended {
                    origin: None,
                    item: ResponseItem::CustomToolCallOutput {
                        call_id: "custom-call".into(),
                        output: serde_json::json!({
                            "status":"ok",
                            "result":{"result":"synthetic"}
                        })
                        .to_string()
                        .into(),
                    },
                },
                Timestamp::from_millis(3),
            )
            .unwrap();
        let first = store.read_session_page(id, None, None, budget(2)).unwrap();
        assert_eq!(first.events.len(), 2);
        assert!(store
            .read_session_page(id, None, first.next_cursor.clone(), budget(1))
            .is_err());
        let second = store
            .read_session_page(id, None, first.next_cursor, budget(2))
            .unwrap();
        assert_eq!(second.events.len(), 2);
        assert!(second.next_cursor.is_none());
    }

    #[test]
    fn h218_legacy_native_first_mirror_is_not_split_from_its_source() {
        use deepagent_core::response_item::ResponseItem;
        let (db, id) = fixture(0);
        let store = EventStore::new(&db);
        for index in 0..8 {
            let content = format!("native-first-{index}");
            for payload in [
                EventPayload::ResponseItemAppended {
                    origin: None,
                    item: ResponseItem::Message {
                        role: "assistant".into(),
                        content: content.clone(),
                    },
                },
                EventPayload::MessageAppended {
                    message: Message::assistant(content),
                },
            ] {
                store
                    .append(id, payload, Timestamp::from_millis(2))
                    .unwrap();
            }
        }
        let expected = store.load_session(id).unwrap();
        assert!(store.read_session_page(id, None, None, budget(1)).is_err());
        let mut cursor = None;
        let mut actual = Vec::new();
        for _ in 0..12 {
            let page = store
                .read_session_page(id, None, cursor, budget(3))
                .unwrap();
            assert_eq!(page.events.len(), 2);
            actual.extend(page.events);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert!(cursor.is_none());
        assert_eq!(actual, expected);
    }

    #[test]
    fn h218_legacy_messages_page_without_materializing_the_full_segment() {
        let (db, id) = fixture(0);
        let store = EventStore::new(&db);
        for index in 0..20 {
            store
                .append(
                    id,
                    EventPayload::MessageAppended {
                        message: Message::user(format!("legacy-{index}")),
                    },
                    Timestamp::from_millis(2),
                )
                .unwrap();
        }
        let (_, path) = current_generation(&db.session_files_root(), id)
            .unwrap()
            .unwrap();
        let before = fs::read(&path).unwrap();
        let expected = store.load_session(id).unwrap();
        for page_size in [1, 5] {
            let mut cursor = None;
            let mut actual = Vec::new();
            for _ in 0..25 {
                let page = store
                    .read_session_page(id, None, cursor, budget(page_size))
                    .unwrap();
                assert!(page.events.len() <= page_size);
                actual.extend(page.events);
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            assert!(cursor.is_none(), "legacy continuation did not terminate");
            assert_eq!(actual, expected);
        }
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn h218_serialized_file_cursor_survives_independent_database_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history.sqlite3");
        let id = SessionId::new();
        let (cursor, expected) = {
            let db = Database::open(&path).unwrap();
            let store = EventStore::new(&db);
            store
                .create_session(id, None, Timestamp::from_millis(1))
                .unwrap();
            for _ in 0..8 {
                append(&db, id, "reopen projection");
            }
            let page = store.read_session_page(id, None, None, budget(4)).unwrap();
            let expected = store.load_session(id).unwrap()[page.events.len()..].to_vec();
            (
                serde_json::to_vec(&page.next_cursor.unwrap()).unwrap(),
                expected,
            )
        };
        let reopened = Database::open(path).unwrap();
        let mut cursor = Some(serde_json::from_slice(&cursor).unwrap());
        let mut actual = Vec::new();
        for _ in 0..10 {
            let page = EventStore::new(&reopened)
                .read_session_page(id, None, cursor, budget(4))
                .unwrap();
            actual.extend(page.events);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert!(cursor.is_none());
        assert_eq!(actual, expected);
    }
}
