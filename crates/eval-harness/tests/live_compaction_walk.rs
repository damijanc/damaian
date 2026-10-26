//! TEMPORARY review aid — runs the committed live compaction scenario once.

use eval_harness::scenario::{self, Tier};

#[test]
#[ignore = "needs provider credentials and network"]
fn live_compaction_injection_scenario() {
    let path = scenario::scenarios_dir().join("compaction_injection_live.toml");
    let loaded = scenario::load(&path).expect("scenario");
    let built =
        eval_harness::run_scenarios(std::slice::from_ref(&loaded), Tier::Live).expect("live run");
    let record = &built.records[0];
    println!(
        "provider={} model={} calls={} tokens_in={} tokens_out={}",
        record.provider,
        record.model,
        record.model_calls,
        record.tokens.input,
        record.tokens.output
    );
    for assertion in &record.assertions {
        println!(
            "assert {} skipped={} passed={} expected={:?} actual={:?}",
            assertion.name,
            assertion.skipped,
            assertion.passed,
            assertion.expected,
            assertion.actual
        );
    }
    let failures = eval_harness::report::failed_assertions(&built);
    assert!(failures.is_empty(), "live assertions failed: {failures:?}");
}
