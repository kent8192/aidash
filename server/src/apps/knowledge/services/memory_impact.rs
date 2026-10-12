//! Authorized impact inspection: which visible Runs and derived Units depend on
//! a Unit, before or after it changes (Issue #127).
use super::native_memory::{bank_provider, policy, scope};
use crate::apps::knowledge::repositories::{access::Lease, memory_impact as repository, units};
use crate::{Error, Result, authorization::identity::Actor, store::Store};
use aidash_domain::memory::impact::{
	AffectedRun, DerivedUnit, Impact, ImpactQuery, Limits, RUN_SCAN_PER_UNIT, RUNS_PER_UNIT,
	UnitImpact, fenced,
};
use std::collections::{BTreeSet, VecDeque};
use uuid::Uuid;

pub async fn impact(store: &Store, actor: &Actor, input: ImpactQuery) -> Result<Impact> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		scope(store, &mut lease, &input.bank, "memory.read").await?;
		bank_provider(&mut lease, &input.bank, &input.provider).await?;
		let policy = policy(&mut lease, &input.provider).await?;
		input.validate(&policy.bounds)?;
		let limits = Limits {
			runs_per_unit: RUNS_PER_UNIT,
			run_scan_per_unit: RUN_SCAN_PER_UNIT,
			derived_visits: policy.bounds.max_graph_visits,
		};
		let mut complete = true;
		let mut units = Vec::with_capacity(input.units.len());
		for id in &input.units {
			// A Unit of another Bank answers exactly like an unknown ID, so the
			// view cannot probe identities outside the authorized Bank.
			let Some(unit) = units::load(&mut lease, *id, false)
				.await?
				.filter(|unit| unit.bank == input.bank)
			else {
				units.push(UnitImpact {
					id: *id,
					readable_revision: None,
					runs: vec![],
					derived: vec![],
				});
				continue;
			};
			let readable_revision = unit.visible().then_some(unit.revision);
			let (runs, runs_complete) =
				runs(store, &mut lease, *id, readable_revision, limits).await?;
			let (derived, derived_complete) =
				derived(store, &mut lease, *id, limits.derived_visits).await?;
			complete &= runs_complete && derived_complete;
			units.push(UnitImpact {
				id: *id,
				readable_revision,
				runs,
				derived,
			});
		}
		Ok(Impact {
			units,
			complete,
			limits,
			observed_at: chrono::Utc::now(),
		})
	}
	.await;
	lease.finish(result).await
}

/// Visible Runs that read `unit`. Hidden Runs are skipped without a trace.
async fn runs(
	store: &Store,
	lease: &mut Lease<'_>,
	unit: Uuid,
	readable_revision: Option<i64>,
	limits: Limits,
) -> Result<(Vec<AffectedRun>, bool)> {
	let mut reads = repository::run_reads(lease, unit, limits.run_scan_per_unit).await?;
	let mut complete = reads.len() <= limits.run_scan_per_unit;
	reads.truncate(limits.run_scan_per_unit);
	let mut runs = Vec::new();
	for (id, read_revision) in reads {
		let Some(run) = repository::run(lease, id).await? else {
			continue;
		};
		if run.home_node != store.node_id || !visible(lease, &run).await? {
			continue;
		}
		if runs.len() == limits.runs_per_unit {
			complete = false;
			break;
		}
		runs.push(AffectedRun {
			id,
			task_id: run.task_id,
			phase: run.phase,
			control: run.control,
			read_revision,
			fenced: fenced(read_revision, readable_revision),
		});
	}
	Ok((runs, complete))
}

/// A Run is listed only if its base disclosure holds for the requester; its
/// read journal may already be fenced, which is what the view reports.
async fn visible(lease: &mut Lease<'_>, run: &aidash_domain::RunMetadata) -> Result<bool> {
	match lease.access() {
		Some(access) => access.run_base_visible(run).await,
		None => Ok(true),
	}
}

/// Live derived Units reachable from `root`, breadth first within the pinned
/// visit bound. Traversal continues through Units the requester cannot see,
/// but only visible ones are reported.
async fn derived(
	store: &Store,
	lease: &mut Lease<'_>,
	root: Uuid,
	cap: usize,
) -> Result<(Vec<DerivedUnit>, bool)> {
	let mut pending = VecDeque::from([root]);
	let mut seen = BTreeSet::from([root]);
	let mut derived = Vec::new();
	while let Some(source) = pending.pop_front() {
		for id in repository::dependents(lease, source, cap).await? {
			if !seen.insert(id) {
				continue;
			}
			if seen.len() > cap + 1 {
				return Ok((derived, false));
			}
			pending.push_back(id);
			let Some(unit) = units::load(lease, id, false).await? else {
				continue;
			};
			if unit.deleted || unit.stale || !readable(store, lease, &unit).await? {
				continue;
			}
			derived.push(DerivedUnit {
				id: unit.id,
				bank: unit.bank,
				revision: unit.revision,
				kind: unit.content.kind,
			});
		}
	}
	Ok((derived, true))
}

async fn readable(
	store: &Store,
	lease: &mut Lease<'_>,
	unit: &aidash_domain::memory::Unit,
) -> Result<bool> {
	match scope(store, lease, &unit.bank, "memory.read").await {
		Ok(()) => Ok(true),
		Err(Error::Forbidden | Error::Conflict(_)) => Ok(false),
		Err(error) => Err(error),
	}
}
