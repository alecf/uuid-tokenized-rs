<div align="center">
  <h1>uuid-tokenized-rs</h1>
  <p>
    <strong>Compact, URL-safe, bijective UUIDs encoded as tokens from an LLM tokenizer's vocabulary</strong>
  </p>
  <p>

[![AGPL License](https://img.shields.io/badge/license-AGPL-blue.svg)](LICENSE)

  </p>
</div>

Encode any 128-bit UUID as exactly eight hyphen-joined lowercase tokens, drawn from a real LLM tokenizer's vocabulary (Gemma, mT5, anything SentencePiece- or BPE-based). The encoding is a **bijection** — no entropy loss, every UUID has exactly one phrase, every valid phrase decodes to exactly one UUID.

```
0ee001c7-12f3-4b29-a4cc-f48838b3587a
       ↓ encode  (Gemma 4 tokenizer.json)
aparte-aceae-ashland-erster-omores-vando-defiant-galactos
       ↓ decode
0ee001c7-12f3-4b29-a4cc-f48838b3587a
```

- URL-safe by construction: lowercase ASCII letters and hyphens only.
- **Pluggable**: works with any Hugging Face `tokenizer.json` or SentencePiece `.model` that has ≥ 65,536 alphabetic 3-to-8-letter tokens after filtering.
- Deterministic: same model file → same encoding, on any machine.
- Zero new heavyweight dependencies (just `serde_json` for the JSON path; the `.model` protobuf path is hand-parsed).

The original sentence-based `generate()` / `short()` / `generate_inverse()` API from `uuid-readable-rs` is still available — see [Legacy: sentence-based encoding](#legacy-sentence-based-encoding) below.

## Quick start (library)

```rust
use uuid::Uuid;
use uuid_readable_rs::UuidCodec;

// Build a codec from a tokenizer file. JSON or .model — auto-detected.
let codec = UuidCodec::from_model_file("path/to/tokenizer.json")?;

let uuid: Uuid = "0ee001c7-12f3-4b29-a4cc-f48838b3587a".parse()?;
let phrase: String = codec.encode(uuid);
// e.g. "aparte-aceae-ashland-erster-omores-vando-defiant-galactos"

let decoded: Uuid = codec.decode(&phrase)?;
assert_eq!(decoded, uuid);
```

`UuidCodec` is reusable — build it once at startup and hold onto it. Loading and filtering a typical 30 MB `tokenizer.json` takes a few hundred milliseconds; encode/decode after that are O(1).

## Quick start (CLI)

```bash
cargo build --release

# Either pass --model each time, or export it once:
export UUID_READABLE_MODEL_PATH=/path/to/tokenizer.json

./target/release/main encode 0ee001c7-12f3-4b29-a4cc-f48838b3587a
# aparte-aceae-ashland-erster-omores-vando-defiant-galactos

./target/release/main decode aparte-aceae-ashland-erster-omores-vando-defiant-galactos
# 0ee001c7-12f3-4b29-a4cc-f48838b3587a
```

## How it works

A 128-bit UUID splits into eight 16-bit chunks (big-endian). Each chunk is an index into a fixed 65,536-entry token table. Hyphen-join the eight tokens; that's the encoding. Decoding inverts the index lookup.

Since 2¹⁶ = 65,536 and 8 × 16 = 128, the mapping is a clean bijection with no padding, no rejection sampling, and no wasted bits.

### Building the token table

The token table is derived deterministically from a tokenizer file:

1. Read every token string from the model file (auto-detected format).
2. Strip the leading word-boundary marker if present (`▁` for SentencePiece, `Ġ` for GPT-2-style BPE).
3. Lowercase.
4. Keep only tokens matching `^[a-z]{3,8}$`. This drops byte tokens, special tokens (`<pad>`, `<eos>`, …), subword fragments with punctuation, and anything that would look ugly in a URL.
5. Sort lexicographically by UTF-8 bytes.
6. Deduplicate.
7. Verify the surviving count is at least 65,536; **truncate to exactly 65,536**.

The pipeline is pure — two consumers of the same model bytes always produce byte-identical encodings.

### Supported tokenizer formats

`UuidCodec::from_model_file` auto-detects the format by the first non-whitespace byte:

- `{` → **Hugging Face `tokenizer.json`** (the JSON HF emits for every tokenizer). Both BPE-shaped vocabs (`model.vocab` as a `{token: id}` object) and Unigram-shaped vocabs (`model.vocab` as a `[[token, score], …]` array) are accepted.
- anything else → **SentencePiece `.model`** protobuf (Google's native format).

### Determinism guarantees

- The same model file always produces the same 65,536-token table — sort, dedup, and truncate are stable.
- Codec construction errors if fewer than 65,536 tokens survive filtering, instead of silently falling back to a smaller table.
- `decode` rejects any input that isn't exactly 8 hyphen-separated tokens, or that contains a token not in the table.

## Getting a tokenizer file

You need any tokenizer with enough alphabetic vocabulary. Some good options:

| Source | Format | Gated | Notes |
|--------|--------|-------|-------|
| `google/gemma-4-E4B` on HF | `tokenizer.json` | yes (free) | Closest to Gemini's tokenizer; 262k vocab |
| `google/gemma-2-2b` on HF | `tokenizer.model` | yes (free) | Smaller download, same Gemma tokenizer family |
| `google/mt5-base` on HF | `spiece.model` | **no** | 250k SentencePiece vocab, zero auth friction |

You only need the tokenizer file (a few MB to ~30 MB), not the model weights. Example:

```bash
# mT5 — no auth required:
curl -L -o mt5.model https://huggingface.co/google/mt5-base/resolve/main/spiece.model

# Gemma — accept license once, then:
curl -L -H "Authorization: Bearer $HF_TOKEN" \
  -o gemma.model \
  https://huggingface.co/google/gemma-2-2b/resolve/main/tokenizer.model
```

> ⚠️ Different tokenizer files produce different encodings. Pick one model file for an application and stick with it — the codec's bijection is between a UUID and a phrase **under a fixed model**.

## Security

This is not a cryptographic primitive — don't use it as a secure random generator. The bijection preserves the entropy of the input UUID and nothing more.

- `UuidCodec::encode` produces 2¹²⁸ distinct phrases (full bijection with UUID).
- The legacy `generate()` produces 25¹² ≈ 5.96×10¹⁶ — also a bijection with the UUID.
- The legacy `short()` collapses 128 bits down to 32 and is **not** a bijection.

## Legacy: sentence-based encoding

The original `uuid-readable-rs` API is still exported unchanged:

```rust
use uuid::Uuid;
use uuid_readable_rs::{generate, generate_from, generate_inverse, short, short_from};

let uuid = Uuid::new_v4();
let long: String = generate_from(uuid);
// e.g. "Wildon Mollie Behka the bubbler of Arecibo moaned Chavey Haney Torbart and 10 calm kingfishers"

let recovered: Uuid = generate_inverse(&long)?;
assert_eq!(uuid, recovered);

let short: String = short_from(uuid);  // 32-bit lossy short form
```

This API uses a built-in vocabulary of names/verbs/places/etc and doesn't require any tokenizer file. The long form is a full bijection with the UUID; the short form is not.

## Credits

Forked from [`uuid-readable-rs`](https://github.com/Martichou/uuid-readable-rs) by Martichou, which itself was inspired by [@Debdut's `uuid-readable`](https://github.com/Debdut/uuid-readable). The tokenized encoding (`UuidCodec`) is new in this fork.
