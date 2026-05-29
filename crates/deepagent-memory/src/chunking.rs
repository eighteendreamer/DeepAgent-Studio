//! Document chunking (the **Chunking** stage of Anthropic's Contextual
//! Retrieval pipeline).
//!
//! Long documents must be split into retrievable units before embedding/BM25
//! indexing. This chunker is markdown-aware: it prefers to split on heading
//! (`#`) and blank-line (paragraph) boundaries, then packs paragraphs into
//! chunks up to a target token budget with a configurable overlap so context is
//! not lost at chunk seams.
//!
//! Token counts use the same word-ish heuristic as the rest of the crate; a
//! real tokenizer can be swapped in without changing the chunk boundaries.

/// A chunk of a source document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// 0-based index of this chunk within its source document.
    pub index: usize,
    /// The heading path this chunk falls under (e.g. `["Setup", "Database"]`),
    /// captured from preceding markdown headings — useful for contextualization.
    pub heading_path: Vec<String>,
    /// The chunk text.
    pub text: String,
}

/// Chunking configuration.
#[derive(Debug, Clone, Copy)]
pub struct ChunkConfig {
    /// Target maximum tokens per chunk.
    pub max_tokens: usize,
    /// Number of trailing tokens of one chunk to prepend to the next (overlap).
    pub overlap_tokens: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            max_tokens: 200,
            overlap_tokens: 30,
        }
    }
}

/// Approximate token count (word count, min 1 for non-empty).
fn count_tokens(text: &str) -> usize {
    let w = text.split_whitespace().count();
    if w == 0 && !text.trim().is_empty() {
        1
    } else {
        w
    }
}

/// Split markdown `doc` into chunks per `config`.
///
/// Algorithm:
/// 1. Walk lines, tracking the current heading path (updated on `#` lines).
/// 2. Accumulate paragraph blocks (separated by blank lines) into the current
///    chunk until adding the next block would exceed `max_tokens`.
/// 3. Emit the chunk, then seed the next chunk with the last `overlap_tokens`
///    words of the emitted text (overlap), and continue.
pub fn chunk_markdown(doc: &str, config: &ChunkConfig) -> Vec<Chunk> {
    let blocks = split_blocks(doc);
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0usize;
    let mut current_heading: Vec<String> = Vec::new();
    let mut chunk_heading: Vec<String> = Vec::new();

    let flush = |chunks: &mut Vec<Chunk>, text: &str, heading: &[String]| -> String {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        chunks.push(Chunk {
            index: chunks.len(),
            heading_path: heading.to_vec(),
            text: trimmed.to_string(),
        });
        // Return the overlap tail (computed by the caller via config).
        trimmed.to_string()
    };

    for block in blocks {
        match block {
            Block::Heading { level, text } => {
                // A heading change starts a new section. Flush the current chunk
                // so each chunk carries a single, coherent heading path (this is
                // what makes the contextualization prefix meaningful).
                if !current.trim().is_empty() {
                    flush(&mut chunks, &current, &chunk_heading);
                    current.clear();
                    current_tokens = 0;
                }
                update_heading_path(&mut current_heading, level, text);
                chunk_heading = current_heading.clone();
            }
            Block::Para(text) => {
                let block_tokens = count_tokens(&text);
                if current_tokens > 0 && current_tokens + block_tokens > config.max_tokens {
                    // Emit current chunk and start a new one with overlap.
                    let emitted = flush(&mut chunks, &current, &chunk_heading);
                    let overlap = tail_words(&emitted, config.overlap_tokens);
                    current = if overlap.is_empty() {
                        String::new()
                    } else {
                        format!("{overlap}\n\n")
                    };
                    current_tokens = count_tokens(&current);
                    chunk_heading = current_heading.clone();
                }
                if current.is_empty() {
                    chunk_heading = current_heading.clone();
                }
                if !current.is_empty() && !current.ends_with('\n') {
                    current.push_str("\n\n");
                }
                current.push_str(&text);
                current_tokens += block_tokens;
            }
        }
    }
    flush(&mut chunks, &current, &chunk_heading);

    // Re-number indices to be contiguous (flush pushes in order already).
    for (i, c) in chunks.iter_mut().enumerate() {
        c.index = i;
    }
    chunks
}

/// The last `n` whitespace-separated words of `text`.
fn tail_words(text: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let start = words.len().saturating_sub(n);
    words[start..].join(" ")
}

enum Block {
    Heading { level: usize, text: String },
    Para(String),
}

fn split_blocks(doc: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para = String::new();

    let flush_para = |blocks: &mut Vec<Block>, para: &mut String| {
        if !para.trim().is_empty() {
            blocks.push(Block::Para(para.trim().to_string()));
        }
        para.clear();
    };

    for line in doc.lines() {
        let trimmed = line.trim_end();
        if let Some((level, text)) = parse_heading(trimmed) {
            flush_para(&mut blocks, &mut para);
            blocks.push(Block::Heading { level, text });
        } else if trimmed.trim().is_empty() {
            flush_para(&mut blocks, &mut para);
        } else {
            if !para.is_empty() {
                para.push('\n');
            }
            para.push_str(trimmed);
        }
    }
    flush_para(&mut blocks, &mut para);
    blocks
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let text = trimmed[level..].trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some((level, text))
    }
}

/// Update the heading path when a new heading of `level` is seen: truncate to
/// the parent depth then push the new heading.
fn update_heading_path(path: &mut Vec<String>, level: usize, text: String) {
    let depth = level.saturating_sub(1);
    path.truncate(depth);
    path.push(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_doc_is_single_chunk() {
        let chunks = chunk_markdown("just a short paragraph", &ChunkConfig::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].index, 0);
        assert_eq!(chunks[0].text, "just a short paragraph");
    }

    #[test]
    fn long_doc_splits_into_multiple_chunks() {
        let para = "word ".repeat(150); // ~150 tokens
        let doc = format!("{para}\n\n{para}\n\n{para}");
        let cfg = ChunkConfig {
            max_tokens: 200,
            overlap_tokens: 10,
        };
        let chunks = chunk_markdown(&doc, &cfg);
        assert!(
            chunks.len() >= 2,
            "expected multiple chunks, got {}",
            chunks.len()
        );
        // Indices are contiguous.
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.index, i);
        }
    }

    #[test]
    fn captures_heading_path() {
        let doc = "# Top\n\nintro text\n\n## Sub\n\ndetail text here";
        let chunks = chunk_markdown(doc, &ChunkConfig::default());
        // The chunk containing "detail" should carry the nested heading path.
        let detail = chunks.iter().find(|c| c.text.contains("detail")).unwrap();
        assert_eq!(
            detail.heading_path,
            vec!["Top".to_string(), "Sub".to_string()]
        );
    }

    #[test]
    fn overlap_carries_context_between_chunks() {
        let a = "alpha ".repeat(120);
        let b = "beta ".repeat(120);
        let doc = format!("{a}\n\n{b}");
        let cfg = ChunkConfig {
            max_tokens: 130,
            overlap_tokens: 15,
        };
        let chunks = chunk_markdown(&doc, &cfg);
        assert!(chunks.len() >= 2);
        // The second chunk should begin with overlap from the first (alpha).
        assert!(chunks[1].text.starts_with("alpha"));
    }

    #[test]
    fn heading_path_truncates_on_shallower_heading() {
        let doc = "# A\n\n## B\n\ntext b\n\n# C\n\ntext c";
        let chunks = chunk_markdown(doc, &ChunkConfig::default());
        let c = chunks.iter().find(|c| c.text.contains("text c")).unwrap();
        assert_eq!(c.heading_path, vec!["C".to_string()]);
    }
}
