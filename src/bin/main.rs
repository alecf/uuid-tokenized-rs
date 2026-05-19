use std::env;
use std::process;

use uuid::Uuid;
use uuid_readable_rs::{generate, generate_from, generate_inverse, short, short_from, UuidCodec};

const MODEL_ENV: &str = "UUID_READABLE_MODEL_PATH";

fn usage() -> ! {
    eprintln!(
        "Usage:
  uuid-readable [<uuid>]                    Long sentence from UUID (random if omitted)
  uuid-readable short [<uuid>]              Short sentence from UUID (random if omitted)
  uuid-readable reverse <sentence>          Recover UUID from a long sentence
  uuid-readable encode --model <path> <uuid>
                                            Compact tokenizer-derived encoding
  uuid-readable decode --model <path> <phrase>
                                            Decode a compact phrase back to a UUID

The encode/decode subcommands also read --model from $UUID_READABLE_MODEL_PATH
when the flag is omitted."
    );
    process::exit(2);
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

fn load_codec(model: Option<String>) -> UuidCodec {
    let path = model.unwrap_or_else(|| {
        eprintln!(
            "no SentencePiece model supplied; pass --model <path> or set ${}",
            MODEL_ENV
        );
        process::exit(2);
    });
    UuidCodec::from_model_file(&path).unwrap_or_else(|e| {
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

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    match args.split_first() {
        None => {
            println!("{}", generate());
        }
        Some((cmd, rest)) if cmd == "short" => match rest.first() {
            None => println!("{}", short()),
            Some(s) => println!("{}", short_from(parse_uuid(s))),
        },
        Some((cmd, rest)) if cmd == "reverse" => {
            if rest.is_empty() {
                usage();
            }
            let sentence = rest.join(" ");
            match generate_inverse(&sentence) {
                Ok(uuid) => println!("{}", uuid),
                Err(e) => {
                    eprintln!("could not reverse sentence: {}", e);
                    process::exit(1);
                }
            }
        }
        Some((cmd, rest)) if cmd == "encode" => {
            let (model, positional) = parse_model_arg(rest);
            let codec = load_codec(model);
            let uuid = match positional.first() {
                Some(s) => parse_uuid(s),
                None => {
                    eprintln!("encode requires a uuid argument");
                    process::exit(2);
                }
            };
            println!("{}", codec.encode(uuid));
        }
        Some((cmd, rest)) if cmd == "decode" => {
            let (model, positional) = parse_model_arg(rest);
            let codec = load_codec(model);
            let phrase = match positional.first() {
                Some(s) => s.clone(),
                None => {
                    eprintln!("decode requires a phrase argument");
                    process::exit(2);
                }
            };
            match codec.decode(&phrase) {
                Ok(uuid) => println!("{}", uuid),
                Err(e) => {
                    eprintln!("could not decode phrase: {:#}", e);
                    process::exit(1);
                }
            }
        }
        Some((cmd, _)) if cmd == "-h" || cmd == "--help" => usage(),
        Some((s, rest)) if rest.is_empty() => {
            println!("{}", generate_from(parse_uuid(s)));
        }
        _ => usage(),
    }
}
