use std::env;
use std::process;

use tokenizers::Tokenizer;
use uuid::Uuid;
use uuid_readable_rs::UuidCodec;

const MODEL_ENV: &str = "UUID_READABLE_MODEL_PATH";

fn usage() -> ! {
    eprintln!(
        "Usage:
  uuid-tokenized encode --model <path> [--verbose] [<uuid>]
                                     Encode a UUID (random if omitted)
  uuid-tokenized decode --model <path> [--verbose] <phrase>
                                     Decode a phrase back to a UUID

Flags:
  --model <path> / -m <path>   Path to a tokenizer.json or SentencePiece .model
  --verbose / -v               Print a comparison of how many tokens the input
                               and output strings tokenize to (tokenizer.json
                               only). Goes to stderr; stdout still emits only
                               the encoded phrase or decoded UUID.

--model also reads from $UUID_READABLE_MODEL_PATH when the flag is omitted."
    );
    process::exit(2);
}

fn extract_bool_flag(args: &mut Vec<String>, long: &str, short: &str) -> bool {
    let mut found = false;
    args.retain(|a| {
        if a == long || a == short {
            found = true;
            false
        } else {
            true
        }
    });
    found
}

fn parse_model_arg(rest: &[String]) -> (Option<String>, Vec<String>) {
    let mut model: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--model" | "-m" => {
                let path = match iter.next() {
                    Some(p) => p.clone(),
                    None => {
                        eprintln!("--model requires a path argument");
                        process::exit(2);
                    }
                };
                model = Some(path);
            }
            other if other.starts_with("--model=") => {
                model = Some(other.trim_start_matches("--model=").to_string());
            }
            _ => positional.push(arg.clone()),
        }
    }
    if model.is_none() {
        model = env::var(MODEL_ENV).ok();
    }
    (model, positional)
}

fn load_codec(model: Option<&str>) -> UuidCodec {
    let path = model.unwrap_or_else(|| {
        eprintln!(
            "no tokenizer model supplied; pass --model <path> or set ${}",
            MODEL_ENV
        );
        process::exit(2);
    });
    UuidCodec::from_model_file(path).unwrap_or_else(|e| {
        eprintln!("failed to load model {}: {:#}", path, e);
        process::exit(1);
    })
}

fn parse_uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).unwrap_or_else(|e| {
        eprintln!("invalid uuid: {}", e);
        process::exit(1);
    })
}

/// Counts tokens in `text` using the given Tokenizer. Returns None on encoding
/// error.
fn count_tokens(tk: &Tokenizer, text: &str) -> Option<usize> {
    tk.encode(text, false).ok().map(|enc| enc.len())
}

/// Print the input/output token-count comparison to stderr.
///
/// Only supports tokenizer.json (the `tokenizers` crate's native format). If
/// the model file is a SentencePiece .model protobuf, the load will fail and
/// we print a one-line warning rather than crashing.
fn print_verbose(model_path: &str, input_label: &str, input: &str, output_label: &str, output: &str) {
    let tk = match Tokenizer::from_file(model_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!();
            eprintln!(
                "warning: --verbose tokenization unavailable: {} (only tokenizer.json is supported)",
                e
            );
            return;
        }
    };
    let in_tokens = count_tokens(&tk, input);
    let out_tokens = count_tokens(&tk, output);

    eprintln!();
    eprintln!(
        "  {:<7} {:>3} chars  →  {}",
        input_label,
        input.len(),
        format_count(in_tokens),
    );
    eprintln!(
        "  {:<7} {:>3} chars  →  {}{}",
        output_label,
        output.len(),
        format_count(out_tokens),
        delta_note(in_tokens, out_tokens),
    );
}

fn delta_note(input: Option<usize>, output: Option<usize>) -> String {
    match (input, output) {
        (Some(i), Some(o)) if i > 0 && i != o => {
            let delta = 100.0 * (o as f64 - i as f64) / i as f64;
            if delta < 0.0 {
                format!("  ({:.0}% fewer)", -delta)
            } else {
                format!("  ({:.0}% more)", delta)
            }
        }
        _ => String::new(),
    }
}

fn format_count(n: Option<usize>) -> String {
    match n {
        Some(n) => format!("{:>3} tokens", n),
        None => "tokenization failed".to_string(),
    }
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let verbose = extract_bool_flag(&mut args, "--verbose", "-v");

    match args.split_first() {
        Some((cmd, rest)) if cmd == "encode" => {
            let (model, positional) = parse_model_arg(rest);
            let codec = load_codec(model.as_deref());
            let uuid = match positional.first() {
                Some(s) => parse_uuid(s),
                None => Uuid::new_v4(),
            };
            let uuid_str = uuid.to_string();
            let phrase = codec.encode(uuid);
            println!("{}", phrase);
            if verbose {
                print_verbose(
                    model.as_deref().expect("load_codec would have exited"),
                    "uuid",
                    &uuid_str,
                    "phrase",
                    &phrase,
                );
            }
        }
        Some((cmd, rest)) if cmd == "decode" => {
            let (model, positional) = parse_model_arg(rest);
            let codec = load_codec(model.as_deref());
            let phrase = match positional.first() {
                Some(s) => s.clone(),
                None => {
                    eprintln!("decode requires a phrase argument");
                    process::exit(2);
                }
            };
            match codec.decode(&phrase) {
                Ok(uuid) => {
                    let uuid_str = uuid.to_string();
                    println!("{}", uuid_str);
                    if verbose {
                        print_verbose(
                            model.as_deref().expect("load_codec would have exited"),
                            "phrase",
                            &phrase,
                            "uuid",
                            &uuid_str,
                        );
                    }
                }
                Err(e) => {
                    eprintln!("could not decode phrase: {:#}", e);
                    process::exit(1);
                }
            }
        }
        Some((cmd, _)) if cmd == "-h" || cmd == "--help" => usage(),
        _ => usage(),
    }
}
