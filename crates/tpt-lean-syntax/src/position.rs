// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offset ↔ line/column mapping. Lines are 1-based. Columns are 0-based and available in
//! UTF-8 bytes, Unicode scalar values (characters), and UTF-16 code units (for LSP).

/// Index of line start offsets for a source text.
#[derive(Debug, Clone)]
pub struct LineIndex {
    starts: Vec<usize>,
    src_len: usize,
    text: String,
}

/// A position in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// 1-based line.
    pub line: usize,
    /// 0-based column in Unicode scalar values.
    pub column: usize,
    /// 0-based column in UTF-16 code units (LSP).
    pub column_utf16: usize,
}

impl LineIndex {
    pub fn new(src: &str) -> Self {
        let mut starts = vec![0];
        let bytes = src.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self {
            starts,
            src_len: src.len(),
            text: src.to_string(),
        }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// Byte offset of the start of a 1-based line.
    pub fn line_start(&self, line: usize) -> Option<usize> {
        line.checked_sub(1).and_then(|i| self.starts.get(i).copied())
    }

    /// 1-based line containing `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }

    /// 0-based column in bytes (used internally for indentation checks).
    pub fn column_of(&self, offset: usize) -> usize {
        let line = self.line_of(offset);
        offset - self.starts[line - 1]
    }

    /// Full position (line, character column, UTF-16 column) for a byte offset.
    pub fn position(&self, offset: usize) -> Position {
        let offset = offset.min(self.src_len);
        let line = self.line_of(offset);
        let start = self.starts[line - 1];
        let slice = &self.text[start..offset];
        Position {
            line,
            column: slice.chars().count(),
            column_utf16: slice.encode_utf16().count(),
        }
    }

    /// Convert a 1-based line and 1-based character column (CLI convention) to a byte offset.
    /// Returns `None` when the line or column is out of range.
    pub fn offset_of(&self, line: usize, column_1based: usize) -> Option<usize> {
        let start = self.line_start(line)?;
        let end = self.line_start(line + 1).unwrap_or(self.src_len);
        let line_text = &self.text[start..end];
        if column_1based == 0 {
            return None;
        }
        let target = column_1based - 1;
        let mut count = 0usize;
        for (i, _) in line_text.char_indices() {
            if count == target {
                return Some(start + i);
            }
            count += 1;
        }
        // Allow the position just past the last character (end of line content).
        let content_chars = line_text.trim_end_matches(['\n', '\r']).chars().count();
        (count == target || target == content_chars).then_some(start + line_text.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_for_ascii() {
        let idx = LineIndex::new("ab\ncd\n");
        assert_eq!(idx.line_count(), 3);
        assert_eq!(
            idx.position(4),
            Position {
                line: 2,
                column: 1,
                column_utf16: 1
            }
        );
        assert_eq!(idx.offset_of(2, 2), Some(4));
    }

    #[test]
    fn utf16_columns_differ_for_astral_chars() {
        let idx = LineIndex::new("😀x");
        let p = idx.position("😀".len());
        assert_eq!(p.column, 1);
        assert_eq!(p.column_utf16, 2);
    }

    #[test]
    fn out_of_range_is_none() {
        let idx = LineIndex::new("a\n");
        assert_eq!(idx.offset_of(9, 1), None);
        assert_eq!(idx.offset_of(1, 0), None);
    }
}
