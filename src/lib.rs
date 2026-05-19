//! Compact, URL-safe, bijective UUID encoding using tokens drawn from an
//! LLM tokenizer's vocabulary.
//!
//! Build a [`UuidCodec`] from a Hugging Face `tokenizer.json` or a
//! SentencePiece `.model` file, then encode any 128-bit UUID as exactly eight
//! hyphen-joined lowercase tokens. The mapping is a bijection — every UUID
//! has exactly one encoding, every valid encoding decodes to exactly one
//! UUID, and the same model file always produces byte-identical output.
//!
//! ```no_run
//! use uuid::Uuid;
//! use uuid_readable_rs::UuidCodec;
//!
//! let codec = UuidCodec::from_model_file("path/to/tokenizer.json")?;
//! let uuid: Uuid = "0ee001c7-12f3-4b29-a4cc-f48838b3587a".parse()?;
//!
//! let phrase = codec.encode(uuid);
//! let decoded = codec.decode(&phrase)?;
//! assert_eq!(uuid, decoded);
//!
//! // Choose your own separator — encode() is just encode_tokens().join("-").
//! let tokens: [String; 8] = codec.encode_tokens(uuid);
//! let smashed: String = tokens.concat();
//! # Ok::<(), anyhow::Error>(())
//! ```

mod codec;

pub use codec::{UuidCodec, PHRASE_LEN, TABLE_SIZE};
