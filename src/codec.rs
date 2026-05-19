//! Compact, URL-friendly UUID encoding using tokens from a SentencePiece model.
//!
//! Builds a deterministic 65,536-entry token table from a SentencePiece `.model`
//! file, then encodes each 128-bit UUID as exactly eight hyphen-joined tokens
//! (16 bits per token). The mapping is a bijection — `decode(encode(uuid)) == uuid`
//! for every UUID, and `encode(decode(s)) == s` for every valid `s`.
//!
//! See `docs/plans/2026-05-18-compact-uuid-encoding-design.md` for the design.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use uuid::Uuid;

/// Size of the token table. Must equal `2^16` so that each of the eight 16-bit
/// chunks of a 128-bit UUID maps to exactly one table entry.
pub const TABLE_SIZE: usize = 1 << 16;

/// Number of tokens in an encoded phrase.
pub const PHRASE_LEN: usize = 8;

/// A codec built from a SentencePiece `.model` file.
///
/// Identical model bytes always produce a codec that encodes and decodes
/// identically — the filter pipeline is pure and deterministic.
#[derive(Debug)]
pub struct UuidCodec {
    tokens: Vec<String>,
    index: HashMap<String, u16>,
}

impl UuidCodec {
    /// Load a codec from a SentencePiece `.model` file on disk.
    pub fn from_model_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let p = path.as_ref();
        let bytes = fs::read(p).with_context(|| format!("reading model file {}", p.display()))?;
        Self::from_model_bytes(&bytes)
    }

    /// Load a codec from the raw bytes of a tokenizer file. The format is
    /// auto-detected:
    ///
    /// - JSON (first non-whitespace byte `{`): parsed as a Hugging Face
    ///   `tokenizer.json`. Both BPE-shaped (`model.vocab` = object) and
    ///   Unigram-shaped (`model.vocab` = array of `[token, score]`) vocabs
    ///   are accepted.
    /// - Anything else: parsed as a SentencePiece `.model` protobuf.
    pub fn from_model_bytes(bytes: &[u8]) -> Result<Self> {
        let raw = if is_json(bytes) {
            parse_tokenizer_json(bytes)?
        } else {
            parse_sentencepiece_model(bytes)?
        };
        let tokens = filter_pieces(raw)?;
        let mut index = HashMap::with_capacity(tokens.len());
        for (i, t) in tokens.iter().enumerate() {
            index.insert(t.clone(), i as u16);
        }
        Ok(Self { tokens, index })
    }

    /// Encode a UUID as exactly [`PHRASE_LEN`] tokens, returned as an array.
    ///
    /// Use this when you want to choose your own separator (or none at all —
    /// `tokens.concat()` smashes them together; `tokens.join("-")` is what
    /// [`encode`](Self::encode) does).
    pub fn encode_tokens(&self, uuid: Uuid) -> [String; PHRASE_LEN] {
        let n = uuid.as_u128();
        std::array::from_fn(|i| {
            let shift = 16 * (PHRASE_LEN - 1 - i);
            let idx = ((n >> shift) & 0xFFFF) as usize;
            self.tokens[idx].clone()
        })
    }

    /// Encode a UUID as a hyphen-joined phrase of exactly [`PHRASE_LEN`] tokens.
    ///
    /// Thin wrapper over [`encode_tokens`](Self::encode_tokens) — equivalent
    /// to `self.encode_tokens(uuid).join("-")`.
    pub fn encode(&self, uuid: Uuid) -> String {
        self.encode_tokens(uuid).join("-")
    }

    /// Decode a hyphen-joined phrase back into the original UUID.
    pub fn decode(&self, phrase: &str) -> Result<Uuid> {
        let parts: Vec<&str> = phrase.split('-').collect();
        if parts.len() != PHRASE_LEN {
            return Err(anyhow!(
                "expected {} hyphen-separated tokens, got {}",
                PHRASE_LEN,
                parts.len()
            ));
        }
        let mut n: u128 = 0;
        for part in &parts {
            let idx = self
                .index
                .get(*part)
                .ok_or_else(|| anyhow!("unknown token: {:?}", part))?;
            n = (n << 16) | (*idx as u128);
        }
        Ok(Uuid::from_u128(n))
    }

    /// Read-only view of the derived token table.
    pub fn tokens(&self) -> &[String] {
        &self.tokens
    }
}

// ---------------------------------------------------------------------------
// SentencePiece protobuf reader
//
// We need only one field path:
//
//   ModelProto.pieces       (field 1, wire type 2 — length-delimited)
//     SentencePiece.piece   (field 1, wire type 2 — length-delimited UTF-8)
//
// All other fields are skipped using the standard wire-type rules described in
// https://protobuf.dev/programming-guides/encoding/
// ---------------------------------------------------------------------------

const WIRE_VARINT: u64 = 0;
const WIRE_I64: u64 = 1;
const WIRE_LEN: u64 = 2;
const WIRE_I32: u64 = 5;

fn is_json(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .map(|b| *b == b'{')
        .unwrap_or(false)
}

fn parse_tokenizer_json(bytes: &[u8]) -> Result<Vec<String>> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).context("parsing tokenizer.json")?;
    let vocab = v
        .get("model")
        .and_then(|m| m.get("vocab"))
        .ok_or_else(|| anyhow!("tokenizer.json: missing 'model.vocab'"))?;
    let mut pieces = Vec::new();
    match vocab {
        // BPE / WordPiece shape: { "▁the": 123, "ing": 456, ... }
        serde_json::Value::Object(map) => {
            for k in map.keys() {
                pieces.push(k.clone());
            }
        }
        // Unigram shape: [ ["<pad>", 0.0], ["▁the", -5.1], ... ]
        serde_json::Value::Array(arr) => {
            for entry in arr {
                if let Some(pair) = entry.as_array() {
                    if let Some(tok) = pair.first().and_then(|x| x.as_str()) {
                        pieces.push(tok.to_string());
                    }
                }
            }
        }
        _ => return Err(anyhow!("tokenizer.json: unexpected 'model.vocab' shape")),
    }
    Ok(pieces)
}

fn parse_sentencepiece_model(bytes: &[u8]) -> Result<Vec<String>> {
    let mut pieces = Vec::new();
    let mut cursor = Cursor::new(bytes);
    while !cursor.is_eof() {
        let tag = cursor.read_varint()?;
        let field = tag >> 3;
        let wire = tag & 7;
        if field == 1 && wire == WIRE_LEN {
            let len = cursor.read_varint()? as usize;
            let submsg = cursor.read_bytes(len)?;
            if let Some(piece) = read_piece_string(submsg)? {
                pieces.push(piece);
            }
        } else {
            cursor.skip_field(wire)?;
        }
    }
    Ok(pieces)
}

fn read_piece_string(bytes: &[u8]) -> Result<Option<String>> {
    let mut cursor = Cursor::new(bytes);
    while !cursor.is_eof() {
        let tag = cursor.read_varint()?;
        let field = tag >> 3;
        let wire = tag & 7;
        if field == 1 && wire == WIRE_LEN {
            let len = cursor.read_varint()? as usize;
            let s_bytes = cursor.read_bytes(len)?;
            return Ok(Some(String::from_utf8_lossy(s_bytes).into_owned()));
        }
        cursor.skip_field(wire)?;
    }
    Ok(None)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn read_varint(&mut self) -> Result<u64> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            if self.is_eof() {
                return Err(anyhow!("unexpected end of input while reading varint"));
            }
            let b = self.bytes[self.pos];
            self.pos += 1;
            result |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift >= 64 {
                return Err(anyhow!("varint too long"));
            }
        }
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos.checked_add(n).map_or(true, |end| end > self.bytes.len()) {
            return Err(anyhow!(
                "attempted to read {} bytes at offset {} but only {} remain",
                n,
                self.pos,
                self.bytes.len().saturating_sub(self.pos)
            ));
        }
        let out = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn skip_field(&mut self, wire: u64) -> Result<()> {
        match wire {
            WIRE_VARINT => {
                self.read_varint()?;
            }
            WIRE_I64 => {
                self.read_bytes(8)?;
            }
            WIRE_LEN => {
                let n = self.read_varint()? as usize;
                self.read_bytes(n)?;
            }
            WIRE_I32 => {
                self.read_bytes(4)?;
            }
            other => return Err(anyhow!("unsupported protobuf wire type {}", other)),
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Filter pipeline
// ---------------------------------------------------------------------------

/// Word-boundary markers used by common tokenizers:
/// - `▁` (U+2581) — SentencePiece
/// - `Ġ` (U+0120) — GPT-2-style BPE
const WORD_MARKERS: &[char] = &['\u{2581}', '\u{0120}'];

fn strip_word_marker(s: &str) -> &str {
    for m in WORD_MARKERS {
        if let Some(rest) = s.strip_prefix(*m) {
            return rest;
        }
    }
    s
}

fn filter_pieces(raw: Vec<String>) -> Result<Vec<String>> {
    let mut filtered: Vec<String> = raw
        .into_iter()
        .filter_map(|p| {
            let trimmed = strip_word_marker(p.as_str());
            let lower = trimmed.to_lowercase();
            if lower.len() < 3 || lower.len() > 8 {
                return None;
            }
            if !lower.bytes().all(|b| b.is_ascii_lowercase()) {
                return None;
            }
            Some(lower)
        })
        .collect();
    filtered.sort();
    filtered.dedup();
    if filtered.len() < TABLE_SIZE {
        return Err(anyhow!(
            "model produced only {} tokens after filtering; need at least {}",
            filtered.len(),
            TABLE_SIZE
        ));
    }
    filtered.truncate(TABLE_SIZE);
    Ok(filtered)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Protobuf encoder helpers (used to synthesize a fake .model file) ---

    fn write_varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push(((v & 0x7F) | 0x80) as u8);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn write_tag(out: &mut Vec<u8>, field: u64, wire: u64) {
        write_varint(out, (field << 3) | wire);
    }

    fn write_len_delim(out: &mut Vec<u8>, field: u64, payload: &[u8]) {
        write_tag(out, field, WIRE_LEN);
        write_varint(out, payload.len() as u64);
        out.extend_from_slice(payload);
    }

    /// Build a fake SentencePiece .model containing the given piece strings.
    /// Each piece is wrapped as a `SentencePiece { piece: "..." }` submessage
    /// inside `ModelProto.pieces`.
    fn synth_model(pieces: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for piece in pieces {
            let mut sub = Vec::new();
            write_len_delim(&mut sub, 1, piece.as_bytes());
            // Also write a fake score (field 2, float = wire type 5) and a fake
            // type enum (field 3, varint) so the skip-unknown paths get exercised.
            write_tag(&mut sub, 2, WIRE_I32);
            sub.extend_from_slice(&[0, 0, 0, 0]);
            write_tag(&mut sub, 3, WIRE_VARINT);
            write_varint(&mut sub, 1);
            write_len_delim(&mut out, 1, &sub);
        }
        // A couple of unknown top-level fields, to exercise skipping.
        write_tag(&mut out, 99, WIRE_VARINT);
        write_varint(&mut out, 42);
        write_tag(&mut out, 100, WIRE_I64);
        out.extend_from_slice(&[0u8; 8]);
        out
    }

    /// Generate enough distinct 3-to-8-letter lowercase ASCII pieces to clear
    /// the 65,536 threshold. We use a deterministic enumeration of 3- and
    /// 4-letter strings — 26^3 + 26^4 = 17,576 + 456,976 = 474,552, more than
    /// enough.
    fn synth_pieces() -> Vec<String> {
        let mut pieces = Vec::new();
        for a in b'a'..=b'z' {
            for b in b'a'..=b'z' {
                for c in b'a'..=b'z' {
                    pieces.push(format!(
                        "{}{}{}",
                        a as char, b as char, c as char
                    ));
                }
            }
        }
        // Add 4-letter variants prefixed with a SentencePiece word-boundary
        // marker, to exercise the marker-stripping path.
        for a in b'a'..=b'z' {
            for b in b'a'..=b'z' {
                for c in b'a'..=b'z' {
                    for d in b'a'..=b'z' {
                        pieces.push(format!(
                            "\u{2581}{}{}{}{}",
                            a as char, b as char, c as char, d as char
                        ));
                        if pieces.len() > TABLE_SIZE + 5000 {
                            return pieces;
                        }
                    }
                }
            }
        }
        pieces
    }

    fn fixture_codec() -> UuidCodec {
        let pieces = synth_pieces();
        let refs: Vec<&str> = pieces.iter().map(String::as_str).collect();
        let bytes = synth_model(&refs);
        UuidCodec::from_model_bytes(&bytes).expect("codec from synthetic model")
    }

    #[test]
    fn table_size_is_exact() {
        let codec = fixture_codec();
        assert_eq!(codec.tokens().len(), TABLE_SIZE);
    }

    #[test]
    fn tokens_are_sorted_and_unique() {
        let codec = fixture_codec();
        let toks = codec.tokens();
        for w in toks.windows(2) {
            assert!(w[0] < w[1], "table not strictly increasing at {:?}", w);
        }
    }

    #[test]
    fn round_trip_zero_uuid() {
        let codec = fixture_codec();
        let u = Uuid::from_u128(0);
        assert_eq!(codec.decode(&codec.encode(u)).unwrap(), u);
    }

    #[test]
    fn round_trip_max_uuid() {
        let codec = fixture_codec();
        let u = Uuid::from_u128(u128::MAX);
        assert_eq!(codec.decode(&codec.encode(u)).unwrap(), u);
    }

    #[test]
    fn round_trip_known_vector() {
        let codec = fixture_codec();
        let u = Uuid::parse_str("0ee001c7-12f3-4b29-a4cc-f48838b3587a").unwrap();
        assert_eq!(codec.decode(&codec.encode(u)).unwrap(), u);
    }

    #[test]
    fn encode_tokens_returns_eight_strings_matching_encode() {
        let codec = fixture_codec();
        let u = Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        let arr = codec.encode_tokens(u);
        assert_eq!(arr.len(), PHRASE_LEN);
        // The joined array must equal the hyphenated form produced by encode().
        assert_eq!(arr.join("-"), codec.encode(u));
        // Caller can also pick a different separator or smash them together.
        let smashed = arr.concat();
        assert!(smashed.bytes().all(|b| b.is_ascii_lowercase()));
        assert_eq!(smashed.len(), arr.iter().map(String::len).sum::<usize>());
    }

    #[test]
    fn encode_emits_eight_hyphenated_tokens() {
        let codec = fixture_codec();
        let s = codec.encode(Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788));
        let parts: Vec<&str> = s.split('-').collect();
        assert_eq!(parts.len(), PHRASE_LEN);
        for p in parts {
            assert!(p.bytes().all(|b| b.is_ascii_lowercase()));
            assert!((3..=8).contains(&p.len()));
        }
    }

    #[test]
    fn round_trip_many_random_uuids() {
        let codec = fixture_codec();
        // Deterministic pseudo-random walk so this test is repeatable.
        let mut n: u128 = 0x9E3779B97F4A7C15_F39CC0605CEDC834;
        for _ in 0..1000 {
            let u = Uuid::from_u128(n);
            assert_eq!(codec.decode(&codec.encode(u)).unwrap(), u, "mismatch at n={n:x}");
            n = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        }
    }

    #[test]
    fn decode_rejects_wrong_word_count() {
        let codec = fixture_codec();
        let full = codec.encode(Uuid::from_u128(0));
        let truncated: String = full.split('-').take(7).collect::<Vec<_>>().join("-");
        assert!(codec.decode(&truncated).is_err());
        let extra = format!("{full}-extra");
        assert!(codec.decode(&extra).is_err());
    }

    #[test]
    fn decode_rejects_unknown_token() {
        let codec = fixture_codec();
        let full = codec.encode(Uuid::from_u128(1));
        let mut parts: Vec<&str> = full.split('-').collect();
        parts[3] = "zzzzzzzz1notatoken";
        let bogus = parts.join("-");
        assert!(codec.decode(&bogus).is_err());
    }

    #[test]
    fn rejects_model_with_too_few_tokens() {
        let pieces: Vec<String> = (0..1000).map(|i| format!("piece{i:04}")).collect();
        let refs: Vec<&str> = pieces.iter().map(String::as_str).collect();
        let bytes = synth_model(&refs);
        let err = UuidCodec::from_model_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("need at least"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn parses_tokenizer_json_bpe_shape() {
        // Build a tokenizer.json with a BPE-shaped vocab containing enough
        // alphabetic tokens to clear TABLE_SIZE.
        let mut vocab = serde_json::Map::new();
        // Special tokens we expect the filter to drop.
        for sp in ["<pad>", "<eos>", "<bos>", "<unk>", "<mask>"] {
            vocab.insert(sp.to_string(), serde_json::json!(vocab.len()));
        }
        for piece in synth_pieces() {
            vocab.insert(piece, serde_json::json!(vocab.len()));
        }
        let body = serde_json::json!({
            "version": "1.0",
            "model": { "type": "BPE", "vocab": vocab, "merges": [] }
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let codec = UuidCodec::from_model_bytes(&bytes).unwrap();
        assert_eq!(codec.tokens().len(), TABLE_SIZE);
        let u = Uuid::from_u128(0xDEADBEEFCAFEBABEu128 << 64 | 0x0123456789ABCDEFu128);
        assert_eq!(codec.decode(&codec.encode(u)).unwrap(), u);
    }

    #[test]
    fn parses_tokenizer_json_unigram_shape() {
        let mut arr: Vec<serde_json::Value> =
            ["<pad>", "<eos>", "<bos>", "<unk>", "<mask>"]
                .iter()
                .map(|s| serde_json::json!([s, 0.0]))
                .collect();
        for piece in synth_pieces() {
            arr.push(serde_json::json!([piece, -1.0]));
        }
        let body = serde_json::json!({
            "version": "1.0",
            "model": { "type": "Unigram", "vocab": arr }
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let codec = UuidCodec::from_model_bytes(&bytes).unwrap();
        assert_eq!(codec.tokens().len(), TABLE_SIZE);
    }

    #[test]
    fn determinism_two_codecs_same_bytes() {
        let pieces = synth_pieces();
        let refs: Vec<&str> = pieces.iter().map(String::as_str).collect();
        let bytes = synth_model(&refs);
        let a = UuidCodec::from_model_bytes(&bytes).unwrap();
        let b = UuidCodec::from_model_bytes(&bytes).unwrap();
        assert_eq!(a.tokens(), b.tokens());
        let u = Uuid::parse_str("0ee001c7-12f3-4b29-a4cc-f48838b3587a").unwrap();
        assert_eq!(a.encode(u), b.encode(u));
    }
}
