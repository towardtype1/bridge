//! The mission plan graph: workstreams and their dependency edges.
//!
//! The Captain produces a `PlanDraft` as structured output (via the CLI's
//! `--json-schema` flag); the harness validates it into a `MissionPlan`
//! with assigned ids. Ops schedules workstreams in topological order.

use crate::ids::{MissionId, WorkstreamId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum PlanError {
    #[error("plan has no workstreams")]
    Empty,
    #[error("duplicate workstream slug: {0}")]
    DuplicateSlug(String),
    #[error("workstream {0} depends on unknown slug {1}")]
    UnknownDependency(String, String),
    #[error("dependency cycle involving workstream {0}")]
    Cycle(String),
    #[error("invalid slug {0}: use lowercase alphanumerics and hyphens")]
    InvalidSlug(String),
}

/// What the Captain emits. Dependencies are expressed by slug because the
/// model does not know harness ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanDraft {
    pub workstreams: Vec<PlanDraftWorkstream>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanDraftWorkstream {
    /// Short kebab-case identifier, used in branch names.
    pub slug: String,
    pub title: String,
    /// Full working brief for the Helm agent.
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl PlanDraft {
    /// JSON Schema handed to `claude --json-schema` for plan output.
    pub fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "workstreams": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "slug": { "type": "string", "pattern": "^[a-z0-9]+(-[a-z0-9]+)*$" },
                            "title": { "type": "string" },
                            "description": { "type": "string" },
                            "depends_on": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["slug", "title", "description"]
                    }
                }
            },
            "required": ["workstreams"]
        })
    }
}

/// One Captain conference turn: conversation plus an optional full plan
/// proposal. The Captain always re-emits the whole desired plan; diffs
/// are computed controller-side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptainReply {
    pub message: String,
    #[serde(default)]
    pub proposed_plan: Option<PlanDraft>,
}

impl CaptainReply {
    /// JSON Schema handed to `claude --json-schema` for every conference turn.
    pub fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" },
                "proposed_plan": {
                    "anyOf": [PlanDraft::json_schema(), { "type": "null" }]
                }
            },
            "required": ["message"]
        })
    }
}

/// Slug-level difference between the current plan and a proposed draft.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlanDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub revised: Vec<String>,
}

impl PlanDiff {
    pub fn between(current: &MissionPlan, draft: &PlanDraft) -> PlanDiff {
        let id_to_slug: HashMap<_, _> = current
            .workstreams
            .iter()
            .map(|w| (w.id, w.slug.clone()))
            .collect();
        let current_deps = |id| -> HashSet<String> {
            current
                .edges
                .iter()
                .filter(|(_, b)| *b == id)
                .map(|(a, _)| id_to_slug[a].clone())
                .collect()
        };
        let draft_by_slug: HashMap<_, _> = draft
            .workstreams
            .iter()
            .map(|w| (w.slug.clone(), w))
            .collect();
        let mut diff = PlanDiff::default();
        for spec in &current.workstreams {
            match draft_by_slug.get(&spec.slug) {
                None => diff.removed.push(spec.slug.clone()),
                Some(d) => {
                    let draft_deps: HashSet<String> = d.depends_on.iter().cloned().collect();
                    if d.title != spec.title
                        || d.description != spec.description
                        || draft_deps != current_deps(spec.id)
                    {
                        diff.revised.push(spec.slug.clone());
                    }
                }
            }
        }
        let current_slugs: HashSet<_> =
            current.workstreams.iter().map(|w| w.slug.clone()).collect();
        for w in &draft.workstreams {
            if !current_slugs.contains(&w.slug) {
                diff.added.push(w.slug.clone());
            }
        }
        diff
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.revised.is_empty()
    }
}

/// A validated plan with harness-assigned workstream ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissionPlan {
    pub mission_id: MissionId,
    /// Kebab-case mission slug, used in branch names.
    pub slug: String,
    pub objective: String,
    pub workstreams: Vec<WorkstreamSpec>,
    /// Edge (a, b) means: a must merge before b may start.
    pub edges: Vec<(WorkstreamId, WorkstreamId)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkstreamSpec {
    pub id: WorkstreamId,
    pub slug: String,
    pub title: String,
    pub description: String,
    /// Ref the worktree branch starts from, normally "main".
    pub base_ref: String,
}

impl WorkstreamSpec {
    pub fn branch_name(&self, mission_slug: &str) -> String {
        format!("bridge/{mission_slug}/{}", self.slug)
    }
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

impl MissionPlan {
    /// Validate a draft: slugs well-formed and unique, dependencies known,
    /// graph acyclic. Assigns fresh workstream ids.
    pub fn from_draft(
        draft: PlanDraft,
        mission_id: MissionId,
        mission_slug: &str,
        objective: &str,
        base_ref: &str,
    ) -> Result<Self, PlanError> {
        if draft.workstreams.is_empty() {
            return Err(PlanError::Empty);
        }
        let mut ids: HashMap<String, WorkstreamId> = HashMap::new();
        for ws in &draft.workstreams {
            if !valid_slug(&ws.slug) {
                return Err(PlanError::InvalidSlug(ws.slug.clone()));
            }
            if ids.insert(ws.slug.clone(), WorkstreamId::new()).is_some() {
                return Err(PlanError::DuplicateSlug(ws.slug.clone()));
            }
        }
        let mut edges = Vec::new();
        for ws in &draft.workstreams {
            for dep in &ws.depends_on {
                let from = *ids
                    .get(dep)
                    .ok_or_else(|| PlanError::UnknownDependency(ws.slug.clone(), dep.clone()))?;
                edges.push((from, ids[&ws.slug]));
            }
        }
        let plan = Self {
            mission_id,
            slug: mission_slug.to_owned(),
            objective: objective.to_owned(),
            workstreams: draft
                .workstreams
                .into_iter()
                .map(|ws| WorkstreamSpec {
                    id: ids[&ws.slug],
                    slug: ws.slug,
                    title: ws.title,
                    description: ws.description,
                    base_ref: base_ref.to_owned(),
                })
                .collect(),
            edges,
        };
        plan.topo_order()?;
        Ok(plan)
    }

    pub fn workstream(&self, id: WorkstreamId) -> Option<&WorkstreamSpec> {
        self.workstreams.iter().find(|w| w.id == id)
    }

    /// Dependencies of `id` that must merge before it starts.
    pub fn dependencies_of(&self, id: WorkstreamId) -> Vec<WorkstreamId> {
        self.edges
            .iter()
            .filter(|(_, to)| *to == id)
            .map(|(from, _)| *from)
            .collect()
    }

    /// Kahn's algorithm. Deterministic: ties broken by declaration order.
    /// A cycle is a `PlanError::Cycle` naming one involved workstream.
    pub fn topo_order(&self) -> Result<Vec<WorkstreamId>, PlanError> {
        let order_index: HashMap<WorkstreamId, usize> = self
            .workstreams
            .iter()
            .enumerate()
            .map(|(i, w)| (w.id, i))
            .collect();
        let mut indegree: HashMap<WorkstreamId, usize> =
            self.workstreams.iter().map(|w| (w.id, 0)).collect();
        let mut adj: HashMap<WorkstreamId, Vec<WorkstreamId>> = HashMap::new();
        let mut seen_edges = HashSet::new();
        for &(from, to) in &self.edges {
            if !seen_edges.insert((from, to)) {
                continue;
            }
            adj.entry(from).or_default().push(to);
            *indegree.entry(to).or_default() += 1;
        }
        let mut ready: VecDeque<WorkstreamId> = {
            let mut v: Vec<_> = indegree
                .iter()
                .filter(|(_, d)| **d == 0)
                .map(|(id, _)| *id)
                .collect();
            v.sort_by_key(|id| order_index[id]);
            v.into()
        };
        let mut out = Vec::with_capacity(self.workstreams.len());
        while let Some(id) = ready.pop_front() {
            out.push(id);
            let mut newly_ready = Vec::new();
            for &next in adj.get(&id).into_iter().flatten() {
                let d = indegree.get_mut(&next).expect("edge target validated");
                *d -= 1;
                if *d == 0 {
                    newly_ready.push(next);
                }
            }
            newly_ready.sort_by_key(|id| order_index[id]);
            ready.extend(newly_ready);
        }
        if out.len() != self.workstreams.len() {
            let stuck = self
                .workstreams
                .iter()
                .find(|w| !out.contains(&w.id))
                .expect("some workstream is stuck in a cycle");
            return Err(PlanError::Cycle(stuck.slug.clone()));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(ws: &[(&str, &[&str])]) -> PlanDraft {
        PlanDraft {
            workstreams: ws
                .iter()
                .map(|(slug, deps)| PlanDraftWorkstream {
                    slug: (*slug).into(),
                    title: format!("title {slug}"),
                    description: format!("desc {slug}"),
                    depends_on: deps.iter().map(|d| (*d).into()).collect(),
                })
                .collect(),
        }
    }

    fn plan(ws: &[(&str, &[&str])]) -> Result<MissionPlan, PlanError> {
        MissionPlan::from_draft(
            draft(ws),
            MissionId::new(),
            "test-mission",
            "objective",
            "main",
        )
    }

    #[test]
    fn linear_chain_orders_correctly() {
        let p = plan(&[("c", &["b"]), ("a", &[]), ("b", &["a"])]).unwrap();
        let order = p.topo_order().unwrap();
        let slug = |id| p.workstream(id).unwrap().slug.clone();
        assert_eq!(
            order.iter().map(|&i| slug(i)).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn independent_workstreams_keep_declaration_order() {
        let p = plan(&[("z", &[]), ("a", &[]), ("m", &[])]).unwrap();
        let order = p.topo_order().unwrap();
        let slugs: Vec<_> = order
            .iter()
            .map(|&i| p.workstream(i).unwrap().slug.clone())
            .collect();
        assert_eq!(slugs, ["z", "a", "m"]);
    }

    #[test]
    fn cycle_is_rejected() {
        let err = plan(&[("a", &["b"]), ("b", &["a"])]).unwrap_err();
        assert!(matches!(err, PlanError::Cycle(_)));
    }

    #[test]
    fn self_dependency_is_a_cycle() {
        let err = plan(&[("a", &["a"])]).unwrap_err();
        assert!(matches!(err, PlanError::Cycle(_)));
    }

    #[test]
    fn unknown_dependency_is_rejected() {
        let err = plan(&[("a", &["ghost"])]).unwrap_err();
        assert_eq!(
            err,
            PlanError::UnknownDependency("a".into(), "ghost".into())
        );
    }

    #[test]
    fn duplicate_slug_is_rejected() {
        let err = plan(&[("a", &[]), ("a", &[])]).unwrap_err();
        assert_eq!(err, PlanError::DuplicateSlug("a".into()));
    }

    #[test]
    fn bad_slugs_are_rejected() {
        for bad in [
            "",
            "UPPER",
            "has space",
            "trailing-",
            "-leading",
            "double--dash",
            "under_score",
        ] {
            let err = plan(&[(bad, &[])]).unwrap_err();
            assert!(
                matches!(err, PlanError::InvalidSlug(_)),
                "slug {bad:?} should be invalid, got {err:?}"
            );
        }
    }

    #[test]
    fn empty_plan_is_rejected() {
        assert_eq!(plan(&[]).unwrap_err(), PlanError::Empty);
    }

    #[test]
    fn branch_names_follow_spec() {
        let p = plan(&[("fix-auth", &[])]).unwrap();
        assert_eq!(
            p.workstreams[0].branch_name(&p.slug),
            "bridge/test-mission/fix-auth"
        );
    }

    #[test]
    fn dependencies_of_reports_incoming_edges() {
        let p = plan(&[("a", &[]), ("b", &[]), ("c", &["a", "b"])]).unwrap();
        let c = p.workstreams[2].id;
        let deps = p.dependencies_of(c);
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn draft_schema_accepts_own_serialization() {
        // The schema must describe what serde produces for PlanDraft.
        let d = draft(&[("a", &[])]);
        let value = serde_json::to_value(&d).unwrap();
        // Cheap structural checks in lieu of a full validator dependency.
        assert!(value["workstreams"].is_array());
        let schema = PlanDraft::json_schema();
        assert_eq!(schema["required"][0], "workstreams");
    }

    #[test]
    fn captain_reply_schema_requires_message_and_embeds_plan() {
        let s = CaptainReply::json_schema();
        assert_eq!(s["required"], serde_json::json!(["message"]));
        let embedded = &s["properties"]["proposed_plan"]["anyOf"][0];
        assert_eq!(*embedded, PlanDraft::json_schema());
    }

    #[test]
    fn captain_reply_parses_with_and_without_plan() {
        let with: CaptainReply = serde_json::from_value(serde_json::json!({
            "message": "Plan ready.",
            "proposed_plan": { "workstreams": [
                { "slug": "a", "title": "A", "description": "Do A", "depends_on": [] }
            ]}
        }))
        .unwrap();
        assert!(with.proposed_plan.is_some());
        let without: CaptainReply =
            serde_json::from_value(serde_json::json!({ "message": "Question?" })).unwrap();
        assert!(without.proposed_plan.is_none());
        let null_plan: CaptainReply =
            serde_json::from_value(serde_json::json!({ "message": "Hmm.", "proposed_plan": null }))
                .unwrap();
        assert!(null_plan.proposed_plan.is_none());
    }

    #[test]
    fn plan_diff_between_classifies_added_removed_revised() {
        let draft_v1 = PlanDraft {
            workstreams: vec![
                PlanDraftWorkstream {
                    slug: "keep".into(),
                    title: "K".into(),
                    description: "d".into(),
                    depends_on: vec![],
                },
                PlanDraftWorkstream {
                    slug: "gone".into(),
                    title: "G".into(),
                    description: "d".into(),
                    depends_on: vec![],
                },
                PlanDraftWorkstream {
                    slug: "edit".into(),
                    title: "E".into(),
                    description: "old".into(),
                    depends_on: vec![],
                },
            ],
        };
        let current =
            MissionPlan::from_draft(draft_v1, MissionId::new(), "m", "obj", "main").unwrap();
        let draft_v2 = PlanDraft {
            workstreams: vec![
                PlanDraftWorkstream {
                    slug: "keep".into(),
                    title: "K".into(),
                    description: "d".into(),
                    depends_on: vec![],
                },
                PlanDraftWorkstream {
                    slug: "edit".into(),
                    title: "E".into(),
                    description: "new".into(),
                    depends_on: vec!["keep".into()],
                },
                PlanDraftWorkstream {
                    slug: "fresh".into(),
                    title: "F".into(),
                    description: "d".into(),
                    depends_on: vec![],
                },
            ],
        };
        let diff = PlanDiff::between(&current, &draft_v2);
        assert_eq!(diff.added, vec!["fresh".to_string()]);
        assert_eq!(diff.removed, vec!["gone".to_string()]);
        assert_eq!(diff.revised, vec!["edit".to_string()]);
        assert!(!diff.is_empty());
    }
}
