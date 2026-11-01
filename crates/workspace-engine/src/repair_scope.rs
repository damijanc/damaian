//! Repair-scope review (spec 102 Task 5).
//!
//! A repair patch is expected to touch the files the task already changed and
//! the files a failing finding points at. Any other path is a scope expansion,
//! and the engine states that as a typed, one-shot review on the patch. The
//! shell and CLI only print these facts; they never decide which paths are in
//! scope or whether repair may proceed.
//!
//! A protected path never reaches this review. Spec 98 refuses the whole
//! proposal at preview time and again at apply time, so there is no card to
//! mark.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::verification::normalized_repository_path;

/// The engine's review of a repair patch's changed paths.
///
/// `expansion_paths` is the fact an approval covers: applying the patch
/// approves those paths for that application only. Nothing stores a reusable
/// grant, so plan, check, baseline and earlier patch approvals cannot stand in
/// for this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairScopeReview {
    /// The engine's heading for the review. A renderer prints it verbatim.
    pub label: String,
    /// The engine's explanation of what the review means and that it is
    /// one-shot.
    pub explanation: String,
    /// True exactly when `expansion_paths` is non-empty. Named explicitly so a
    /// renderer never has to derive the fact from a path count.
    pub one_shot: bool,
    /// Every path the review considers already in scope, sorted.
    pub existing_paths: Vec<String>,
    /// Every proposed path outside the existing scope, sorted.
    pub expansion_paths: Vec<String>,
    /// Why each path group is in the review.
    pub reason_groups: Vec<RepairScopeReasonGroup>,
    /// What the card may tell the user an approval does.
    pub actions: Vec<RepairScopeAction>,
}

/// One reason group in a repair-scope review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairScopeReasonGroup {
    pub kind: RepairScopeReasonKind,
    /// The engine's group heading.
    pub label: String,
    /// The engine's explanation of why these paths belong here.
    pub explanation: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairScopeReasonKind {
    /// Paths the task already changed before this repair.
    TaskChangedPath,
    /// Paths named by normalized finding evidence in the active report.
    FindingEvidencePath,
    /// Proposed paths in neither of the two groups above.
    OutsideExistingScope,
}

/// One engine-owned action a repair-scope card may render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairScopeAction {
    pub kind: RepairScopeActionKind,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairScopeActionKind {
    ApproveScopeExpansionOnce,
}

impl RepairScopeReview {
    /// Compute the review from the task's changed paths, the active report's
    /// normalized finding paths and the proposed patch paths.
    ///
    /// All three are normalized with the same repository-path rule the
    /// fingerprint uses, so `./src/a.rs` and `src/a.rs` cannot both appear, and
    /// an absolute finding path is reduced to repository content before
    /// comparison.
    pub fn compute(
        task_changed_paths: &[String],
        finding_paths: &[String],
        proposed_paths: &[String],
    ) -> Self {
        let task_changed = normalized_set(task_changed_paths);
        let findings = normalized_set(finding_paths);
        let proposed = normalized_set(proposed_paths);

        let existing: BTreeSet<String> = task_changed.union(&findings).cloned().collect();
        let expansions: Vec<String> = proposed.difference(&existing).cloned().collect();

        let mut reason_groups = Vec::new();
        if !task_changed.is_empty() {
            reason_groups.push(RepairScopeReasonGroup {
                kind: RepairScopeReasonKind::TaskChangedPath,
                label: "Task-changed paths".to_string(),
                explanation: "The task already changed these paths before this repair.".to_string(),
                paths: task_changed.iter().cloned().collect(),
            });
        }
        if !findings.is_empty() {
            reason_groups.push(RepairScopeReasonGroup {
                kind: RepairScopeReasonKind::FindingEvidencePath,
                label: "Finding locations".to_string(),
                explanation:
                    "The failing findings in this report point at these paths, so a repair may touch them."
                        .to_string(),
                paths: findings.iter().cloned().collect(),
            });
        }
        if !expansions.is_empty() {
            reason_groups.push(RepairScopeReasonGroup {
                kind: RepairScopeReasonKind::OutsideExistingScope,
                label: "Outside the existing repair scope".to_string(),
                explanation:
                    "These paths are neither task-changed paths nor finding locations. Applying this patch approves this expansion once; it does not create a standing permission."
                        .to_string(),
                paths: expansions.clone(),
            });
        }

        let one_shot = !expansions.is_empty();
        let actions = if one_shot {
            vec![RepairScopeAction {
                kind: RepairScopeActionKind::ApproveScopeExpansionOnce,
                label: "Apply this patch only".to_string(),
                detail: "Approval covers these paths for this patch application alone. Plan, check, baseline and earlier patch approvals do not grant them."
                    .to_string(),
            }]
        } else {
            Vec::new()
        };

        Self {
            label: "Repair scope expansion".to_string(),
            explanation: if one_shot {
                "This repair patch changes paths outside the task's changed files and failing findings. The review marks the expansion, and approval applies to this patch once."
                    .to_string()
            } else {
                "This repair patch changes only paths in the task's existing repair scope."
                    .to_string()
            },
            one_shot,
            existing_paths: existing.into_iter().collect(),
            expansion_paths: expansions,
            reason_groups,
            actions,
        }
    }

    /// The review to attach to a patch, or `None` when the patch stays inside
    /// the existing scope. An ordinary patch never calls this at all.
    pub fn for_patch(
        task_changed_paths: &[String],
        finding_paths: &[String],
        proposed_paths: &[String],
    ) -> Option<Self> {
        let review = Self::compute(task_changed_paths, finding_paths, proposed_paths);
        review.one_shot.then_some(review)
    }

    /// The CLI's rendering of these facts. The engine owns every word; a
    /// caller prints the result and adds no wording of its own.
    pub fn render(&self) -> String {
        let mut text = format!("{}\n{}\n", self.label, self.explanation);
        for group in &self.reason_groups {
            text.push_str(&format!(
                "{}: {}\n  {}\n",
                group.label,
                group.paths.join(", "),
                group.explanation
            ));
        }
        for action in &self.actions {
            text.push_str(&format!("{} — {}\n", action.label, action.detail));
        }
        text
    }
}

fn normalized_set(paths: &[String]) -> BTreeSet<String> {
    paths
        .iter()
        .map(|path| normalized_repository_path(path))
        .filter(|path| !path.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_changed_and_finding_paths_are_existing_scope() {
        let review = RepairScopeReview::compute(
            &["src/app.rs".to_string()],
            &["src/report.rs".to_string()],
            &["src/app.rs".to_string(), "src/report.rs".to_string()],
        );
        assert!(!review.one_shot);
        assert!(review.expansion_paths.is_empty());
        assert_eq!(
            review.existing_paths,
            vec!["src/app.rs".to_string(), "src/report.rs".to_string()]
        );
        assert!(review.actions.is_empty());
    }

    #[test]
    fn a_new_path_is_a_one_shot_expansion() {
        let review = RepairScopeReview::compute(
            &["./src/app.rs".to_string()],
            &["src/report.rs".to_string()],
            &["src/new.rs".to_string()],
        );
        assert!(review.one_shot);
        assert_eq!(review.expansion_paths, vec!["src/new.rs".to_string()]);
        assert_eq!(
            review.existing_paths,
            vec!["src/app.rs".to_string(), "src/report.rs".to_string()]
        );
        assert_eq!(review.actions.len(), 1);
        assert_eq!(
            review.actions[0].kind,
            RepairScopeActionKind::ApproveScopeExpansionOnce
        );
        assert!(review.reason_groups.iter().any(|group| {
            group.kind == RepairScopeReasonKind::OutsideExistingScope
                && group.paths == vec!["src/new.rs".to_string()]
        }));
    }

    #[test]
    fn path_spelling_does_not_create_an_expansion() {
        let review = RepairScopeReview::compute(
            &["src/app.rs".to_string()],
            &[],
            &["./src/app.rs".to_string()],
        );
        assert!(!review.one_shot, "{review:?}");
    }

    #[test]
    fn an_earlier_approval_is_not_an_input_to_a_later_review() {
        let changed = ["src/app.rs".to_string()];
        let first = RepairScopeReview::compute(&changed, &[], &["src/new.rs".to_string()]);
        assert!(first.one_shot);
        // Nothing from the first review is passed in: a new patch's expansion
        // is recomputed from paths alone, so approval never becomes a grant.
        let later = RepairScopeReview::compute(&changed, &[], &["src/other.rs".to_string()]);
        assert!(later.one_shot);
        assert_eq!(later.expansion_paths, vec!["src/other.rs".to_string()]);
    }

    #[test]
    fn render_prints_only_engine_owned_facts() {
        let review = RepairScopeReview::compute(
            &["src/app.rs".to_string()],
            &["src/report.rs".to_string()],
            &["src/new.rs".to_string()],
        );
        let text = review.render();
        assert!(text.contains(&review.label), "{text}");
        assert!(text.contains(&review.explanation), "{text}");
        for group in &review.reason_groups {
            assert!(text.contains(&group.label), "{text}");
            assert!(text.contains(&group.explanation), "{text}");
            for path in &group.paths {
                assert!(text.contains(path), "{text}");
            }
        }
        for action in &review.actions {
            assert!(text.contains(&action.label), "{text}");
            assert!(text.contains(&action.detail), "{text}");
        }
    }
}
