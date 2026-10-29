//! One content-identity check for Canvas materialization. The caller holds the
//! existing owner/state/path gate. Bytes are hashed and collected in one pass;
//! path-only consumers discard each verified chunk instead of buffering media.

use super::{ArtifactRecord, CoreError, Digest, File, Read, Result, Sha256, COPY_BUFFER_BYTES};

fn mismatch(record: &ArtifactRecord, reason: impl Into<String>) -> CoreError {
    CoreError::ArtifactIntegrityMismatch {
        artifact_id: record.id.clone(),
        expected_digest: record.digest.clone(),
        reason: reason.into(),
    }
}

fn expected_identity(record: &ArtifactRecord) -> Result<(u64, &str)> {
    let bytes = u64::try_from(record.byte_size)
        .ok()
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| mismatch(record, "invalid published byte count"))?;
    let digest = record
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| mismatch(record, "missing or invalid published SHA-256"))?;
    Ok((bytes, digest))
}

pub(super) fn read_bytes(record: &ArtifactRecord) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    verify_file(record, Some(&mut bytes))?;
    Ok(bytes)
}

pub(super) fn verify_file(record: &ArtifactRecord, output: Option<&mut Vec<u8>>) -> Result<()> {
    let (expected_bytes, _) = expected_identity(record)?;
    let mut file = File::open(&record.path)
        .map_err(|error| mismatch(record, format!("unreadable artifact: {error}")))?;
    let metadata = file
        .metadata()
        .map_err(|error| mismatch(record, format!("unreadable artifact metadata: {error}")))?;
    if !metadata.is_file() || metadata.len() != expected_bytes {
        return Err(mismatch(
            record,
            "published file type or byte count changed",
        ));
    }
    verify_reader(record, &mut file, output)
}

fn verify_reader(
    record: &ArtifactRecord,
    reader: &mut impl Read,
    mut output: Option<&mut Vec<u8>>,
) -> Result<()> {
    let (expected_bytes, expected_digest) = expected_identity(record)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; COPY_BUFFER_BYTES];
    loop {
        // One extra byte detects growth without reading an unbounded suffix,
        // even when another process continuously appends to the open file.
        let limit = (expected_bytes - total + 1).min(buffer.len() as u64) as usize;
        let count = match reader.read(&mut buffer[..limit]) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(mismatch(record, format!("unreadable artifact: {error}"))),
        };
        total += count as u64;
        if total > expected_bytes {
            return Err(mismatch(record, "published byte count increased"));
        }
        digest.update(&buffer[..count]);
        if let Some(bytes) = &mut output {
            bytes
                .try_reserve(count)
                .map_err(|error| mismatch(record, format!("cannot buffer artifact: {error}")))?;
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
    if total != expected_bytes
        || !format!("{:x}", digest.finalize()).eq_ignore_ascii_case(expected_digest)
    {
        return Err(mismatch(record, "published byte count or SHA-256 changed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h250_growing_reader_stops_at_expected_length_plus_one_without_buffering() {
        struct GrowingReader(usize);
        impl Read for GrowingReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                assert!(buffer.len() <= COPY_BUFFER_BYTES);
                buffer.fill(b'x');
                self.0 += buffer.len();
                Ok(buffer.len())
            }
        }
        let record = ArtifactRecord {
            id: "art_h250".into(),
            kind: super::super::ArtifactKind::Video,
            run_id: None,
            call_id: None,
            workspace_id: None,
            path: "unused-reader-fixture".into(),
            media_type: None,
            byte_size: (COPY_BUFFER_BYTES * 3) as i64,
            digest: Some(format!("sha256:{:x}", Sha256::digest(b"original"))),
            created_at: 1,
        };
        let mut reader = GrowingReader(0);
        assert!(matches!(
            verify_reader(&record, &mut reader, None).unwrap_err(),
            CoreError::ArtifactIntegrityMismatch { .. }
        ));
        assert_eq!(reader.0 as i64, record.byte_size + 1);
    }
}
