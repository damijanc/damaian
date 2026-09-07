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
    let mut records = Vec::new();
    for scenario in scenario::load_all()? {
        if scenario.tier != tier && scenario.blocked_on.is_none() {
            continue;
        }
        let run = runner::run(&scenario)?;
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
        }
        records.push(record);
    }
    Ok(report::build(records))
}
