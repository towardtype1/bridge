//! Pure amendment validation: diff a re-emitted full PlanDraft against the
//! current MissionPlan under the lock rules. No IO, no controller state.

use bridge_core::{MissionId, MissionPlan, PlanDiff, PlanDraft, WorkstreamId, WorkstreamStatus};
use std::collections::{HashMap, HashSet};

pub fn validate_amendment(
    plan: &MissionPlan,
    statuses: &HashMap<WorkstreamId, WorkstreamStatus>,
    draft: &PlanDraft,
) -> Result<PlanDiff, String> {
    // Structural validity first (slugs, unknown deps, cycles).
    MissionPlan::from_draft(
        draft.clone(),
        MissionId::new(),
        &plan.slug,
        &plan.objective,
        "validation",
    )
    .map_err(|e| e.to_string())?;

    let id_to_slug: HashMap<_, _> = plan
        .workstreams
        .iter()
        .map(|w| (w.id, w.slug.as_str()))
        .collect();
    let draft_by_slug: HashMap<_, _> = draft
        .workstreams
        .iter()
        .map(|w| (w.slug.as_str(), w))
        .collect();

    for spec in &plan.workstreams {
        let locked = statuses
            .get(&spec.id)
            .is_none_or(|s| !matches!(s, WorkstreamStatus::Pending));
        if !locked {
            continue;
        }
        let Some(d) = draft_by_slug.get(spec.slug.as_str()) else {
            return Err(format!(
                "workstream \"{}\" is locked (already started or finished) and cannot be removed",
                spec.slug
            ));
        };
        let current_deps: HashSet<&str> = plan
            .edges
            .iter()
            .filter(|(_, b)| *b == spec.id)
            .map(|(a, _)| id_to_slug[a])
            .collect();
        let draft_deps: HashSet<&str> = d.depends_on.iter().map(String::as_str).collect();
        if d.title != spec.title || d.description != spec.description || draft_deps != current_deps
        {
            return Err(format!(
                "workstream \"{}\" is locked (already started or finished) and cannot be edited",
                spec.slug
            ));
        }
    }

    let diff = PlanDiff::between(plan, draft);
    if diff.is_empty() {
        return Err("amendment changes nothing".into());
    }
    Ok(diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::*;
    use std::collections::HashMap;

    fn plan_and_statuses(
        specs: &[(&str, &[&str])],
        statuses: &[(&str, WorkstreamStatus)],
    ) -> (MissionPlan, HashMap<WorkstreamId, WorkstreamStatus>) {
        let draft = PlanDraft {
            workstreams: specs
                .iter()
                .map(|(slug, deps)| PlanDraftWorkstream {
                    slug: (*slug).into(),
                    title: format!("T {slug}"),
                    description: format!("D {slug}"),
                    depends_on: deps.iter().map(|d| (*d).to_string()).collect(),
                })
                .collect(),
        };
        let plan = MissionPlan::from_draft(draft, MissionId::new(), "m", "o", "main").unwrap();
        let map = statuses
            .iter()
            .map(|(slug, st)| {
                let id = plan
                    .workstreams
                    .iter()
                    .find(|w| w.slug == *slug)
                    .unwrap()
                    .id;
                (id, st.clone())
            })
            .collect();
        (plan, map)
    }

    fn draft(specs: &[(&str, &str, &[&str])]) -> PlanDraft {
        PlanDraft {
            workstreams: specs
                .iter()
                .map(|(slug, desc, deps)| PlanDraftWorkstream {
                    slug: (*slug).into(),
                    title: format!("T {slug}"),
                    description: (*desc).to_string(),
                    depends_on: deps.iter().map(|d| (*d).to_string()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn adding_a_workstream_is_ok() {
        let (plan, st) = plan_and_statuses(&[("run", &[])], &[("run", WorkstreamStatus::Working)]);
        let d = draft(&[("run", "D run", &[]), ("new", "D new", &[])]);
        let diff = validate_amendment(&plan, &st, &d).unwrap();
        assert_eq!(diff.added, vec!["new".to_string()]);
        assert!(diff.removed.is_empty() && diff.revised.is_empty());
    }

    #[test]
    fn revising_pending_is_ok_but_locked_is_rejected() {
        let (plan, st) = plan_and_statuses(
            &[("run", &[]), ("wait", &[])],
            &[
                ("run", WorkstreamStatus::Working),
                ("wait", WorkstreamStatus::Pending),
            ],
        );
        let ok = draft(&[("run", "D run", &[]), ("wait", "new brief", &[])]);
        assert_eq!(
            validate_amendment(&plan, &st, &ok).unwrap().revised,
            vec!["wait".to_string()]
        );
        let bad = draft(&[("run", "EDITED", &[]), ("wait", "D wait", &[])]);
        let err = validate_amendment(&plan, &st, &bad).unwrap_err();
        assert!(err.contains("run"), "error names the locked slug: {err}");
    }

    #[test]
    fn removing_locked_rejected_removing_pending_ok() {
        let (plan, st) = plan_and_statuses(
            &[("run", &[]), ("wait", &[])],
            &[
                ("run", WorkstreamStatus::Merged),
                ("wait", WorkstreamStatus::Pending),
            ],
        );
        let bad = draft(&[("wait", "D wait", &[])]);
        assert!(
            validate_amendment(&plan, &st, &bad).is_err(),
            "merged must remain"
        );
        let ok = draft(&[("run", "D run", &[])]);
        assert_eq!(
            validate_amendment(&plan, &st, &ok).unwrap().removed,
            vec!["wait".to_string()]
        );
    }

    #[test]
    fn removing_pending_with_survivor_dependent_rejected() {
        let (plan, st) = plan_and_statuses(
            &[("base", &[]), ("dep", &["base"])],
            &[
                ("base", WorkstreamStatus::Pending),
                ("dep", WorkstreamStatus::Pending),
            ],
        );
        let bad = draft(&[("dep", "D dep", &["base"])]); // base removed, dep survives
        assert!(validate_amendment(&plan, &st, &bad).is_err());
    }

    #[test]
    fn empty_diff_rejected_and_cycles_rejected() {
        let (plan, st) = plan_and_statuses(&[("a", &[])], &[("a", WorkstreamStatus::Pending)]);
        let same = draft(&[("a", "D a", &[])]);
        assert!(
            validate_amendment(&plan, &st, &same)
                .unwrap_err()
                .contains("nothing")
        );
        let cyc = draft(&[("a", "D a", &["b"]), ("b", "D b", &["a"])]);
        assert!(validate_amendment(&plan, &st, &cyc).is_err());
    }
}
