//! The shared strict authority walk. Replay collects groups; optional search
//! stages their projection in its existing transaction. A visitor must not
//! publish anything before this walk succeeds (the final CRC can still fail).
use super::*;

pub(crate) struct ScannedSessionFile {
    pub generation: u64,
    pub header: SessionFileHeader,
    pub last_sequence: Option<u64>,
    pub byte_len: u64,
    pub recovered_tail_bytes: u64,
    pub updated_at: Timestamp,
    pub ended_at: Option<Timestamp>,
}

pub(crate) type GroupVisitor<'a> = dyn FnMut(Vec<Event>, Option<ProjectionSpan>) -> Result<()> + 'a;

pub(crate) fn read(
    file: &mut File,
    generation: u64,
    expected: SessionId,
    repair_tail: bool,
    control: &ReadControl<'_>,
    visitor: &mut GroupVisitor<'_>,
) -> std::result::Result<ScannedSessionFile, MaintenanceReadError> {
    control.checkpoint()?;
    file.seek(SeekFrom::Start(0))?;
    let mut state = RecordState {
        generation,
        expected,
        header: None,
        next_sequence: 0,
        updated_at: None,
        ended_at: None,
    };
    let mut input = Vec::new();
    let mut position = 0;
    let mut eof = false;
    let mut valid_len = 0u64;
    loop {
        control.checkpoint()?;
        let remaining = &input[position..];
        let Some(size) = frame_size(remaining, valid_len)? else {
            if eof {
                break;
            }
            control.source_size(remaining.len() as u64)?;
            if position > 0 {
                input.drain(..position);
                position = 0;
            }
            let mut chunk = [0u8; 64 * 1024];
            let read = file.read(&mut chunk)?;
            control.source_bytes(read)?;
            input.extend_from_slice(&chunk[..read]);
            eof = read == 0;
            continue;
        };
        control.source_size(size as u64)?;
        visit_frame(&remaining[..size], valid_len, &mut state, control, visitor)?;
        valid_len = valid_len
            .checked_add(size as u64)
            .ok_or_else(|| CoreError::EventLog("session frame size overflow".into()))?;
        position += size;
    }
    let header = state
        .header
        .ok_or_else(|| CoreError::EventLog(format!("empty session file for {expected}")))?;
    let torn_tail = (input.len() - position) as u64;
    control.checkpoint()?;
    if repair_tail && torn_tail > 0 {
        file.set_len(valid_len)?;
        file.sync_all()?;
    }
    Ok(ScannedSessionFile {
        generation,
        updated_at: state.updated_at.unwrap_or(header.created_at),
        header,
        last_sequence: state.next_sequence.checked_sub(1),
        byte_len: valid_len,
        recovered_tail_bytes: torn_tail,
        ended_at: state.ended_at,
    })
}

fn frame_size(bytes: &[u8], offset: u64) -> Result<Option<usize>> {
    if bytes.is_empty() || (bytes.len() < ZSTD_MAGIC.len() && ZSTD_MAGIC.starts_with(bytes)) {
        return Ok(None);
    }
    match zstd::zstd_safe::find_frame_compressed_size(bytes) {
        Ok(size) if size <= bytes.len() => Ok(Some(size)),
        Ok(_) => Ok(None),
        Err(code) if bytes.starts_with(&ZSTD_MAGIC) && is_truncated_frame(code) => Ok(None),
        Err(code) => Err(CoreError::EventLog(format!(
            "invalid zstd frame at byte {offset}: {}",
            zstd::zstd_safe::get_error_name(code)
        ))),
    }
}

struct RecordState {
    generation: u64,
    expected: SessionId,
    header: Option<SessionFileHeader>,
    next_sequence: u64,
    updated_at: Option<Timestamp>,
    ended_at: Option<Timestamp>,
}

impl RecordState {
    fn visit(
        &mut self,
        line: &[u8],
        offset: u64,
        visitor: &mut GroupVisitor<'_>,
    ) -> std::result::Result<(), MaintenanceReadError> {
        if line.is_empty() {
            return Ok(());
        }
        let record = serde_json::from_slice(line).map_err(|error| {
            CoreError::EventLog(format!("invalid JSONL record at byte {offset}: {error}"))
        })?;
        if self.header.is_none() {
            self.header = Some(parse_header(Some(record), self.generation, self.expected)?);
            return Ok(());
        }
        let (group, projection) = parse_event_group(record, self.expected, self.next_sequence)?;
        self.next_sequence = self
            .next_sequence
            .checked_add(group.len() as u64)
            .ok_or_else(|| CoreError::EventLog("session sequence overflow".into()))?;
        for event in &group {
            self.updated_at = Some(event.timestamp);
            if let Some(ended) = event.payload.session_lifecycle_ended() {
                self.ended_at = ended.then_some(event.timestamp);
            }
        }
        visitor(group, projection).map_err(MaintenanceReadError::Projection)
    }
}

fn visit_frame(
    bytes: &[u8],
    offset: u64,
    state: &mut RecordState,
    control: &ReadControl<'_>,
    visitor: &mut GroupVisitor<'_>,
) -> std::result::Result<(), MaintenanceReadError> {
    let corrupt =
        |error| CoreError::EventLog(format!("corrupt zstd frame at byte {offset}: {error}"));
    let mut decoder = zstd::stream::read::Decoder::new(bytes).map_err(corrupt)?;
    let mut line = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        control.checkpoint()?;
        let read = decoder.read(&mut chunk).map_err(corrupt)?;
        if read == 0 {
            break;
        }
        control.decoded_bytes(read)?;
        for part in chunk[..read].split_inclusive(|byte| *byte == b'\n') {
            control.checkpoint()?;
            let newline = part.last() == Some(&b'\n');
            let part = if newline {
                &part[..part.len() - 1]
            } else {
                part
            };
            let size = line
                .len()
                .checked_add(part.len())
                .ok_or_else(|| CoreError::EventLog("session record size overflow".into()))?;
            control.record_size(size as u64)?;
            line.extend_from_slice(part);
            if newline {
                state.visit(&line, offset, visitor)?;
                line.clear();
            }
        }
    }
    control.checkpoint()?;
    state.visit(&line, offset, visitor)
}
