//! Text chunking for ingestion: fixed-size, overlap-aware, paragraph-boundary
//! preferring splitter.

/// Split `text` into chunks of ~`size` characters with `overlap` overlap,
/// preferring paragraph and sentence boundaries over hard cuts.
pub fn chunk_text(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let size = size.max(64);
    let overlap = overlap.min(size / 2);

    let mut chunks = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    let boundary = |pos: usize| -> usize {
        // Try to snap forward to the end of a paragraph or sentence.
        let window = &text[pos..(pos + 300).min(text.len())];
        match window.find("\n\n").or_else(|| window.find(". ")) {
            Some(i) => pos + i + 1,
            None => pos,
        }
    };

    while start < bytes.len() {
        let mut end = (start + size).min(bytes.len());
        if end < bytes.len() {
            end = boundary(end - overlap);
        }
        // Always snap to a UTF-8 char boundary.
        while end < bytes.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        let mut s = start;
        while s < end && !text.is_char_boundary(s) {
            s += 1;
        }
        let chunk = text[s..end].trim();
        if !chunk.is_empty() {
            chunks.push(chunk.to_string());
        }
        if end >= bytes.len() {
            break;
        }
        start = end - overlap;
        while start < bytes.len() && !text.is_char_boundary(start) {
            start += 1;
        }
    }
    chunks
}

/// Build the citation-aware retrieval context block injected into the LLM
/// prompt. Kept *stable in ordering* (highest score first, then chunk index)
/// so identical documents yield byte-identical prompts, maximizing vLLM
/// prefix-cache block reuse.
pub fn format_context_block(hits: &[crate::rag::qdrant::SearchHit]) -> String {
    let mut out = String::from("SUPPORT DOCUMENTS\n=================\n");
    for (i, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "[{}] (source: {}, score: {:.3})\n{}\n\n",
            i + 1,
            hit.payload.source,
            hit.score,
            hit.payload.text
        ));
    }
    out
}
