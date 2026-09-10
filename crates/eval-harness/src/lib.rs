//! The Damaian local evaluation harness. See
//! `docs/specs/18_local_evaluation_harness/proposal.md`.

pub mod assertions;
pub mod fixture;
pub mod guard;
pub mod metrics;
pub mod record;
pub mod report;
pub mod runner;
pub mod scenario;
pub mod trace;

use workspace_engine::Result;

use crate::scenario::Tier;

/// Runs every committed scenario for a tier and builds its report. Both the
/// binary and the CI test call this, so there is one implementation of "run the
/// tier" rather than two that drift (§5.1).
///
/// A blocked scenario is included whatever the tier: it reports
/// `notApplicable`, and leaving it out would make the gap invisible in exactly
/// the output that is supposed to carry it.
pub fn run_tier(tier: Tier) -> Result<report::Report> {
    run_scenarios(&scenario::load_all()?, tier)
}

/// The scenarios a tier runs.
///
/// §5.3: "The live tier runs the same scenario files against a real provider,
/// ignoring the `[[turn]]` scripts and keeping the `[assert]` block." So the
/// `tier` field names where a scenario is *defined* — it is what tells the
/// deterministic tier to skip a scenario that has no script to replay — and it
/// is not a filter the live tier applies to itself. Reading it as one left the
/// live tier selecting nothing, since every committed scenario declares
/// `deterministic`.
pub fn selected_for(scenarios: &[scenario::Scenario], tier: Tier) -> Vec<&scenario::Scenario> {
    scenarios
        .iter()
        .filter(|scenario| {
            tier == Tier::Live || scenario.tier == tier || scenario.blocked_on.is_some()
        })
        .collect()
}

/// [`run_tier`] over a given set of scenarios, so the selection and the
/// empty-run refusal can be exercised without the committed scenario files.
pub fn run_scenarios(scenarios: &[scenario::Scenario], tier: Tier) -> Result<report::Report> {
    let selected = selected_for(scenarios, tier);
    // A report over zero records is not a passing run: it has no failing
    // assertions, so the binary exits 0 and every metric reports no data. That
    // is honest but useless, and it is what the live tier did for its whole
    // existence. Refuse loudly instead.
    if selected.is_empty() {
        return Err(workspace_engine::ClientError::InvalidInput(format!(
            "the {} tier selected no scenarios; a run that measures nothing must not report success",
            tier.as_str()
        )));
    }

    let mut records = Vec::new();
    for scenario in selected {
        let run = match tier {
            Tier::Deterministic => runner::run(scenario)?,
            Tier::Live => runner::run_live(scenario)?,
        };
        let patch_paths: Vec<String> = run
            .patch_proposal
            .as_ref()
            .map(|proposal| {
                proposal
                    .files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut record = run.record.clone();
        if record.not_applicable.is_none() {
            record.assertions = assertions::evaluate(&run, &scenario.asserts, tier, &patch_paths);
            // Sanitized again *here*, after the assertions are attached. The
            // pass inside `runner::drive` runs before this point, so it sees an
            // empty assertion list — leaving assertion text as the one route by
            // which a secret could reach the report and the committed baseline.
            record.sanitize(&workspace_engine::SecretScanner::default());
        }
        records.push(record);
    }
    Ok(report::build(records))
}
