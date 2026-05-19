//! End-to-end test: build a synthetic SentencePiece .model file, then drive
//! the `main` CLI binary against it for encode + decode round-trips.
//!
//! This is the only test that exercises both the protobuf reader *and* the
//! shipped binary together, so it catches integration regressions that the
//! library-level unit tests would miss.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use uuid::Uuid;

const PHRASE_LEN: usize = 8;
const TABLE_SIZE: usize = 1 << 16;

// --- minimal protobuf writer (mirrors the test helpers in src/codec.rs) ----

const WIRE_VARINT: u64 = 0;
const WIRE_LEN: u64 = 2;
const WIRE_I32: u64 = 5;

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

fn synth_model(pieces: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for piece in pieces {
        let mut sub = Vec::new();
        write_len_delim(&mut sub, 1, piece.as_bytes());
        write_tag(&mut sub, 2, WIRE_I32);
        sub.extend_from_slice(&[0, 0, 0, 0]);
        write_tag(&mut sub, 3, WIRE_VARINT);
        write_varint(&mut sub, 1);
        write_len_delim(&mut out, 1, &sub);
    }
    out
}

fn synth_pieces() -> Vec<String> {
    let mut pieces = Vec::new();
    for a in b'a'..=b'z' {
        for b in b'a'..=b'z' {
            for c in b'a'..=b'z' {
                pieces.push(format!("{}{}{}", a as char, b as char, c as char));
            }
        }
    }
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

// ---------------------------------------------------------------------------

fn cli_binary() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by Cargo for integration tests; the binary
    // here is `main` (from src/bin/main.rs).
    let path = env!("CARGO_BIN_EXE_main");
    PathBuf::from(path)
}

static FIXTURE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn write_fixture_model() -> PathBuf {
    let mut path = env::temp_dir();
    let n = FIXTURE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    path.push(format!(
        "uuid-readable-rs-test-{}-{}.model",
        std::process::id(),
        n
    ));
    let bytes = synth_model(&synth_pieces());
    fs::write(&path, bytes).expect("write fixture model");
    path
}

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(cli_binary())
        .args(args)
        .output()
        .expect("spawn cli");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn run_cli_no_model_env(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(cli_binary())
        .env_remove("UUID_READABLE_MODEL_PATH")
        .args(args)
        .output()
        .expect("spawn cli");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn cli_encode_decode_roundtrip() {
    let model = write_fixture_model();
    let model_str = model.to_str().unwrap();

    let uuid = "0ee001c7-12f3-4b29-a4cc-f48838b3587a";

    let (encoded, stderr, code) = run_cli(&["encode", "--model", model_str, uuid]);
    assert_eq!(code, 0, "encode failed: {}", stderr);
    let phrase = encoded.trim().to_string();
    let parts: Vec<&str> = phrase.split('-').collect();
    assert_eq!(parts.len(), PHRASE_LEN, "phrase: {:?}", phrase);
    for p in &parts {
        assert!(
            p.bytes().all(|b| b.is_ascii_lowercase()),
            "non-lowercase-alpha token: {:?}",
            p
        );
        assert!((3..=8).contains(&p.len()), "wrong-length token: {:?}", p);
    }

    let (decoded, stderr, code) = run_cli(&["decode", "--model", model_str, &phrase]);
    assert_eq!(code, 0, "decode failed: {}", stderr);
    assert_eq!(decoded.trim(), uuid);

    let _ = fs::remove_file(&model);
}

#[test]
fn cli_encode_requires_model() {
    let (_stdout, stderr, code) =
        run_cli_no_model_env(&["encode", "0ee001c7-12f3-4b29-a4cc-f48838b3587a"]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("no tokenizer model supplied")
            || stderr.contains("UUID_READABLE_MODEL_PATH"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn cli_encode_rejects_missing_model_file() {
    let (_stdout, stderr, code) = run_cli(&[
        "encode",
        "--model",
        "/definitely/does/not/exist.model",
        "0ee001c7-12f3-4b29-a4cc-f48838b3587a",
    ]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("failed to load model"),
        "stderr: {}",
        stderr
    );
}

#[test]
fn cli_verbose_warns_when_tokenization_unavailable() {
    // Our synthetic fixture is a SentencePiece .model protobuf, which the
    // `tokenizers` crate can't load (it expects tokenizer.json). The CLI
    // should still emit the encoded phrase to stdout, and warn on stderr.
    let model = write_fixture_model();
    let model_str = model.to_str().unwrap();
    let (stdout, stderr, code) = run_cli(&[
        "encode",
        "--model",
        model_str,
        "--verbose",
        "0ee001c7-12f3-4b29-a4cc-f48838b3587a",
    ]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    let phrase = stdout.trim();
    assert_eq!(phrase.split('-').count(), PHRASE_LEN);
    assert!(
        stderr.contains("--verbose tokenization unavailable")
            || stderr.contains("tokenizer.json"),
        "expected unavailable warning, got: {}",
        stderr
    );
    let _ = fs::remove_file(&model);
}

#[test]
fn cli_encode_with_no_uuid_generates_random() {
    let model = write_fixture_model();
    let model_str = model.to_str().unwrap();
    let (stdout, stderr, code) = run_cli(&["encode", "--model", model_str]);
    assert_eq!(code, 0, "stderr: {}", stderr);
    let phrase = stdout.trim();
    let parts: Vec<&str> = phrase.split('-').collect();
    assert_eq!(parts.len(), PHRASE_LEN);
    let (decoded, _, code) = run_cli(&["decode", "--model", model_str, phrase]);
    assert_eq!(code, 0);
    assert!(Uuid::parse_str(decoded.trim()).is_ok());
    let _ = fs::remove_file(&model);
}
