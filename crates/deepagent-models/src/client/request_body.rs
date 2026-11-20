//! Final DeepSeek wire budget, shared by all protocols and send entries.
use deepagent_core::error::{CoreError, Result};
use serde::Serialize;
use std::io::{self, Write};

// https://api-docs.deepseek.com/guides/vision/#limits
// Applies to the complete JSON body, including inline base64, on every route.
const MAX_REQUEST_BYTES: usize = 48 * 1024 * 1024;

pub(super) struct ChatRequest<'a> {
    pub request: &'a crate::ChatCompletionRequest,
    pub deepseek_extensions: bool,
}

impl Serialize for ChatRequest<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.request.serialize_for_provider(
            serializer,
            self.request
                .reasoning_effort
                .or_else(|| {
                    self.request
                        .thinking
                        .as_ref()
                        .filter(|thinking| thinking.kind == "disabled")
                        .map(|_| crate::ReasoningEffort::None)
                })
                .filter(|effort| {
                    !self.deepseek_extensions || *effort != crate::ReasoningEffort::None
                })
                .map(crate::ReasoningEffort::label),
            self.deepseek_extensions
                .then_some(self.request.thinking.as_ref())
                .flatten(),
            self.deepseek_extensions,
        )
    }
}

pub(crate) fn encode(request: &impl Serialize) -> Result<String> {
    encode_with_limit(request, MAX_REQUEST_BYTES)
}

fn encode_with_limit(request: &impl Serialize, limit: usize) -> Result<String> {
    let mut output = BoundedBody {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    if let Err(error) = serde_json::to_writer(&mut output, request) {
        if output.exceeded {
            return Err(CoreError::invalid(
                "request_body_too_large: DeepSeek request exceeds 48 MiB; reduce input before retrying",
            ));
        }
        return Err(error.into());
    }
    String::from_utf8(output.bytes)
        .map_err(|error| CoreError::Serialization(format!("request JSON is not UTF-8: {error}")))
}

struct BoundedBody {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for BoundedBody {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.bytes.len() {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request body limit exceeded",
            ));
        }
        let needed = self.bytes.len() + bytes.len();
        if needed > self.bytes.capacity() {
            // Retain amortized growth for escaped text without growing this
            // output buffer beyond the budget. Input/projection memory is not
            // bounded here; the existing request serializers still own it.
            let capacity = needed
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::SerializeSeq;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn exact_utf8_and_escaped_json_bytes_determine_the_limit() {
        let value = serde_json::json!({"text":"中文 🦀\n\"\\", "items":[1,2,3]});
        let expected = serde_json::to_string(&value).unwrap();
        let size = expected.len();
        assert!(size > expected.chars().count());
        for limit in [size, size + 1] {
            assert_eq!(encode_with_limit(&value, limit).unwrap(), expected);
        }
        assert!(matches!(
            encode_with_limit(&value, size - 1),
            Err(CoreError::Invalid(_))
        ));
        assert!(matches!(
            encode_with_limit(&value, 0),
            Err(CoreError::Invalid(_))
        ));
    }

    #[test]
    fn overflow_stops_serialization_and_does_not_append_the_rejected_chunk() {
        struct Sequence<'a>(&'a AtomicUsize);
        impl Serialize for Sequence<'_> {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                let mut sequence = serializer.serialize_seq(Some(1_000))?;
                for _ in 0..1_000 {
                    self.0.fetch_add(1, Ordering::SeqCst);
                    sequence.serialize_element("escaped\nvalue")?;
                }
                sequence.end()
            }
        }
        let visited = AtomicUsize::new(0);
        assert!(encode_with_limit(&Sequence(&visited), 64).is_err());
        assert!(visited.load(Ordering::SeqCst) < 10);
        let mut output = BoundedBody {
            bytes: Vec::new(),
            limit: 8,
            exceeded: false,
        };
        output.write_all(b"12345678").unwrap();
        assert!(output.write_all(b"9").is_err());
        assert_eq!(output.bytes, b"12345678");
        assert!(output.bytes.capacity() <= output.limit);
    }

    #[test]
    fn unrelated_serializer_errors_keep_their_category_and_context() {
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(
                &self,
                _: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("synthetic serialization failure"))
            }
        }
        let error = encode(&Invalid).unwrap_err();
        assert!(matches!(error, CoreError::Serialization(_)));
        assert!(error
            .to_string()
            .contains("synthetic serialization failure"));
        assert!(!error.to_string().contains("request_body_too_large"));
    }
}
