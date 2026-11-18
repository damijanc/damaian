use super::*;
use workspace_engine::CostEstimate;

fn usage_with(cached: Option<u64>, reported_input: u64, silent_runs: u32) -> TaskUsage {
    TaskUsage {
        input_tokens: 10_000,
        output_tokens: 500,
        source: UsageSource::Measured,
        reported_cost: None,
        run_count: 2,
        cached_input_tokens: cached,
        cache_reported_input_tokens: reported_input,
        runs_without_cache_report: silent_runs,
        models: Vec::new(),
    }
}

fn json_for(usage: TaskUsage, estimate: Option<CostEstimate>) -> String {
    task_usage_json(Some(&usage), estimate)
}

/// A real upper-bound estimate, built through `Config::estimated_cost`
/// rather than a constructor: `CostEstimate`'s are crate-private to
/// the engine on purpose, so nothing outside it can mint a figure and
/// call it exact.
fn upper_bound_estimate() -> CostEstimate {
    let mut config = Config::default();
    config.apply_overlay(
        workspace_engine::ConfigOverlay::parse(concat!(
            "model_provider=deepseek\n",
            "model_provider.deepseek.price_per_million_input_tokens=1.0\n",
            "model_provider.deepseek.price_per_million_output_tokens=1.0\n",
        ))
        .unwrap(),
    );
    let estimate = config
        .estimated_cost(&workspace_engine::TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            cached_input_tokens: Some(500_000),
            source: UsageSource::Measured,
        })
        .expect("both base rates are set");
    assert!(estimate.is_upper_bound(), "no cached rate is configured");
    estimate
}

#[test]
fn a_reported_split_carries_the_count_and_the_rate() {
    let json = json_for(usage_with(Some(6_000), 8_000, 0), None);

    assert!(json.contains("\"cachedInputTokens\":6000"), "{json}");
    // 6000 of the 8000 input tokens whose runs reported a split — not
    // of the task's 10000, which would dilute the rate with runs
    // nobody can see into.
    assert!(json.contains("\"cacheHitRate\":0.75"), "{json}");
    assert!(json.contains("\"runsWithoutCacheReport\":0"), "{json}");
}

#[test]
fn an_unreported_split_omits_the_fields_rather_than_zeroing_them() {
    let json = json_for(usage_with(None, 0, 2), None);

    assert!(!json.contains("cachedInputTokens"), "{json}");
    assert!(!json.contains("cacheHitRate"), "{json}");
    // The count of silent runs is still carried: it is what lets the
    // UI say "not reported" rather than guess.
    assert!(json.contains("\"runsWithoutCacheReport\":2"), "{json}");
}

#[test]
fn a_reported_zero_is_a_rate_of_zero_not_an_absent_field() {
    // The provider measured that none of it hit. That is a fact and
    // renders as 0%, unlike silence, which renders as "not reported".
    let json = json_for(usage_with(Some(0), 8_000, 0), None);

    assert!(json.contains("\"cachedInputTokens\":0"), "{json}");
    assert!(json.contains("\"cacheHitRate\":0"), "{json}");
}

#[test]
fn a_partial_report_says_how_many_runs_it_covers() {
    let json = json_for(usage_with(Some(500), 1_000, 3), None);

    assert!(json.contains("\"cacheHitRate\":0.5"), "{json}");
    assert!(json.contains("\"runsWithoutCacheReport\":3"), "{json}");
}

#[test]
fn a_zero_denominator_yields_no_rate_rather_than_a_division_by_zero() {
    // Reachable: a run refused by the provider records measured zero,
    // whose `cached_input_tokens` is `Some(0)` with no input tokens.
    let json = json_for(usage_with(Some(0), 0, 0), None);

    assert!(!json.contains("cacheHitRate"), "{json}");
    assert!(json.contains("\"cachedInputTokens\":0"), "{json}");
}

#[test]
fn an_upper_bound_cost_says_so() {
    let json = json_for(
        usage_with(Some(6_000), 8_000, 0),
        Some(upper_bound_estimate()),
    );

    assert!(json.contains("\"estimatedCost\":1"), "{json}");
    assert!(
        json.contains("\"estimatedCostIsUpperBound\":true"),
        "{json}"
    );
}
