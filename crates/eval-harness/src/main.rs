use eval_harness::scenario::Tier;
use eval_harness::{report, run_tier};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let tier = match value_of(&arguments, "--tier").as_deref() {
        Some("live") => Tier::Live,
        Some("deterministic") | None => Tier::Deterministic,
        Some(other) => {
            eprintln!("unknown tier `{other}`: expected `deterministic` or `live`");
            std::process::exit(2);
        }
    };
    let format = value_of(&arguments, "--format").unwrap_or_else(|| "text".to_string());

    let built = match run_tier(tier) {
        Ok(built) => built,
        Err(error) => {
            eprintln!("the evaluation could not run: {error:?}");
            std::process::exit(1);
        }
    };

    match format.as_str() {
        "json" => match report::to_json(&built) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("{error:?}");
                std::process::exit(1);
            }
        },
        "text" => print!("{}", report::to_text(&built)),
        other => {
            eprintln!("unknown format `{other}`: expected `text` or `json`");
            std::process::exit(2);
        }
    }

    // A non-zero exit on failure is what makes this usable from a script.
    if !report::failed_assertions(&built).is_empty() {
        std::process::exit(1);
    }
}

/// Reads `--flag value`. A leading positional `run` is simply ignored, which
/// keeps §5.1's documented invocation
/// (`cargo run -p eval-harness -- run --tier deterministic`) working while also
/// accepting the same command without the subcommand.
fn value_of(arguments: &[String], flag: &str) -> Option<String> {
    let position = arguments.iter().position(|argument| argument == flag)?;
    arguments.get(position + 1).cloned()
}
