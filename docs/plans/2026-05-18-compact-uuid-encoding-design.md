# Compact UUID Encoding (SentencePiece-derived)

Date: 2026-05-18

## Problem

The existing `generate()` / `generate_inverse()` API maps a 128-bit UUID to/from a
grammatically structured sentence. Two issues:

1. The sentence is verbose (~14 space-separated words, ~80–100 characters) because
   roughly a third of the tokens are grammatical filler ("the", "of", "and", "by").
2. The output is not URL-friendly: it contains spaces, mixed case, and a digit run.

We want a representation that:

- is a **bijection with the original 128-bit UUID** (no entropy loss, no rejection
  sampling),
- is URL-safe: lowercase, hyphen-separated, alphabetic only,
- is substantially shorter than the long sentence,
- draws its vocabulary from a real LLM tokenizer (so the tokens are "things a
  modern tokenizer already understands"), and
- is **pluggable**: the model file backing the vocabulary can be swapped without
  recompiling the crate.

## Approach

Build a 65,536-entry token table by deterministically filtering a SentencePiece
`.model` file, then encode each UUID as exactly 8 hyphen-joined tokens.

### Encoding

```
uuid (128 bits, big-endian) → [u16; 8] → [TOKENS[i] for i in chunks].join("-")
```

Example output shape: `lemon-cargo-vivid-thatch-quiet-river-stone-onyx`.

8 tokens × 16 bits = 128 bits exactly. Each 16-bit chunk indexes into a fixed
65,536-entry table, so the mapping is a bijection — every UUID has exactly one
encoding, every valid encoding decodes to exactly one UUID.

### Token table (derived from a SentencePiece model)

The token table is computed on first use from a SentencePiece `.model` file
supplied by the caller. The filter pipeline is pure and deterministic:

1. Parse the protobuf, collecting every `SentencePiece.piece` string.
2. Strip the SentencePiece word-boundary marker `▁` (U+2581) from the start of
   each piece.
3. Lowercase.
4. Keep only pieces matching the regex `^[a-z]{3,8}$`. This drops byte tokens
   (`<0x00>`), control tokens (`<unk>`, `<s>`), punctuation, numbers, and
   pieces that would be ugly in URLs.
5. Sort lexicographically (UTF-8 byte order).
6. Deduplicate adjacent duplicates.
7. Verify the surviving count is at least 65,536. Error otherwise.
8. Truncate to exactly 65,536 entries.

The same `.model` file always produces the same table because every step above
is deterministic.

### Pluggability

The library does not bundle a model file. Callers supply one via:

- `UuidCodec::from_model_file(path)` — read from disk.
- `UuidCodec::from_model_bytes(bytes)` — read from memory (useful when the model
  is embedded with `include_bytes!`).

The CLI accepts `--model <path>` and falls back to the `UUID_READABLE_MODEL_PATH`
environment variable.

Recommended sources (any SentencePiece `.model` with ≥ 65,536 alphabetic
3-to-8-letter pieces works):

- **Gemma 2 / Gemma 3** (`google/gemma-2-2b` on Hugging Face): 256k vocab,
  closest public analog to Gemini's tokenizer. Gated — user must accept the
  license once.
- **mT5** (`google/mt5-base`): 250k multilingual SentencePiece vocab, not
  gated.
- **T5 v1.1** (`google/t5-v1_1-base`): 32k vocab — too small to reach 65,536
  after filtering. Not recommended.

## Why no static `tokens.rs`

An earlier sketch had a one-off generator script producing a Rust source file
with the literal 65,536-token list, checked into the repo. The pluggable design
supersedes that:

- Switching tokenizers requires no codegen rerun or commit churn.
- The crate stays small (no embedded vocab).
- The model file is the canonical source of truth at runtime, not a derived
  artifact from a past run.

## Parsing the `.model` file

SentencePiece `.model` files are protobuf-encoded `ModelProto` messages. We
need exactly one field path:

```
ModelProto.pieces[]  (field 1, wire type 2 — length-delimited)
  .piece             (field 1, wire type 2 — length-delimited UTF-8 string)
```

A handwritten minimal protobuf reader (~60 lines) walks the top-level message,
recurses one level into each `pieces` submessage, and extracts the `piece`
string. All other fields are skipped using the standard wire-format skip rules
for the four wire types we may encounter (varint, fixed32, fixed64,
length-delimited). This avoids depending on `prost`, `protobuf`, or the C++
`sentencepiece` crate.

## API

```rust
pub struct UuidCodec { /* tokens: [String; 65536], index: HashMap<String, u16> */ }

impl UuidCodec {
    pub fn from_model_file<P: AsRef<Path>>(path: P) -> Result<Self>;
    pub fn from_model_bytes(bytes: &[u8]) -> Result<Self>;
    pub fn encode(&self, uuid: Uuid) -> String;
    pub fn decode(&self, s: &str) -> Result<Uuid>;
}
```

The existing `generate` / `generate_from` / `generate_inverse` / `short` /
`short_from` functions are unchanged. The new codec is an additional surface.

## Errors

`from_model_*` returns `Err` for:

- malformed protobuf,
- fewer than 65,536 surviving tokens.

`decode` returns `Err` for:

- input not containing exactly 8 hyphen-separated tokens,
- a token not present in the table.

## Testing

Two layers:

1. **Synthetic-fixture tests (always run).** A helper constructs a valid
   SentencePiece `.model` byte buffer in memory containing ~70,000 fake pieces
   (`"aaa"`, `"aab"`, …). The codec is built from those bytes and exercised
   with: all-zero UUID, all-ones UUID, known test vector, 1,000 random
   round-trips, decode-rejection cases (wrong word count, unknown token,
   trailing hyphen).
2. **Real-model integration test (opt-in).** Reads
   `UUID_READABLE_MODEL_PATH`; skipped if unset. Confirms a real Gemma/mT5
   `.model` produces ≥ 65,536 tokens and round-trips correctly.

## Out of scope

- Multi-language tokenization (we discard non-ASCII pieces by design).
- Memorability scoring / curated word lists.
- Versioning of the table independent of the model file. Different model files
  produce different encodings; callers are responsible for using a consistent
  model end-to-end.
