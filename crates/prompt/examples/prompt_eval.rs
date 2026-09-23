//! Offline prompt-eval harness (spec §8). No network, no provider.
//!
//! `cargo run -p callcore-prompt --example prompt_eval [-- path/to/fixtures.json]`
//!
//! Builds the exact prompt for every fixture x answer style, prints a size
//! table (chars and a rough `chars/4` token estimate) and writes each built
//! prompt to `crates/prompt/eval/out/<fixture-id>.<style>.txt` (gitignored) so
//! prompt changes can be diffed by eye. Live answer scoring is done by a
//! separate tool with `callcore_prompt::eval::check_answer`.

use callcore_prompt::eval::{estimate_tokens, EvalSuite};
use callcore_prompt::{style_id, ANSWER_STYLES};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval");
    let fixtures_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("fixtures.json"));
    let json = match std::fs::read_to_string(&fixtures_path) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("cannot read {}: {e}", fixtures_path.display());
            return ExitCode::FAILURE;
        }
    };
    let suite = match EvalSuite::from_json(&json) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let out_dir = root.join("out");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("cannot create {}: {e}", out_dir.display());
        return ExitCode::FAILURE;
    }

    println!(
        "{:<28} {:<14} {:<9} {:>7} {:>7} {:>6} {:>7} {:>7}",
        "fixture", "call_type", "style", "prefix", "suffix", "user", "total", "~tokens"
    );
    println!("{}", "-".repeat(92));
    let (mut total_chars, mut total_tokens, mut count) = (0usize, 0usize, 0usize);
    let mut max_tokens = 0usize;
    for fixture in &suite.fixtures {
        for style in ANSWER_STYLES {
            let Some(parts) = suite.build(fixture, style) else {
                eprintln!("fixture {} has no profile", fixture.id);
                return ExitCode::FAILURE;
            };
            let p = parts.cached_prefix.chars().count();
            let s = parts.style_suffix.chars().count();
            let u = parts.user_message.chars().count();
            let total = p + s + u;
            let tokens = estimate_tokens(&parts.cached_prefix)
                + estimate_tokens(&parts.style_suffix)
                + estimate_tokens(&parts.user_message);
            println!(
                "{:<28} {:<14} {:<9} {:>7} {:>7} {:>6} {:>7} {:>7}",
                fixture.id,
                fixture.call_type.id(),
                style_id(style),
                p,
                s,
                u,
                total,
                tokens
            );
            total_chars += total;
            total_tokens += tokens;
            max_tokens = max_tokens.max(tokens);
            count += 1;

            let file = out_dir.join(format!("{}.{}.txt", fixture.id, style_id(style)));
            let body = format!(
                "=== cached_prefix ===\n{}\n=== style_suffix ===\n{}\n=== user_message ===\n{}\n",
                parts.cached_prefix, parts.style_suffix, parts.user_message
            );
            if let Err(e) = std::fs::write(&file, body) {
                eprintln!("cannot write {}: {e}", file.display());
                return ExitCode::FAILURE;
            }
        }
    }
    println!("{}", "-".repeat(92));
    println!(
        "{count} prompts from {} fixtures: {total_chars} chars, ~{total_tokens} input tokens total, \
         ~{} avg, ~{max_tokens} max",
        suite.fixtures.len(),
        total_tokens / count.max(1)
    );
    println!("wrote {}", out_dir.display());
    ExitCode::SUCCESS
}
