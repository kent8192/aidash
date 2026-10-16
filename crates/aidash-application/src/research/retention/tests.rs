use super::*;
use crate::ports::research::RetainedSnapshot;
use aidash_domain::retention::SnapshotPeriods;
use async_trait::async_trait;
use chrono::TimeZone;
use std::sync::Mutex;

fn hours(hours: i64) -> DateTime<Utc> {
	Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap() + Duration::hours(hours)
}

fn rules(after_run_hours: u32, max_age_hours: u32) -> RetentionRules {
	RetentionRules {
		document_snapshots: SnapshotPeriods {
			after_run_seconds: after_run_hours * 3_600,
			max_age_seconds: max_age_hours * 3_600,
		},
	}
}

struct Snapshots {
	all: Vec<RetainedSnapshot>,
	purged: Mutex<Vec<Uuid>>,
	/// Every `(retrieved_before, ended_before)` the sweeper asked with.
	asked: Mutex<Vec<(DateTime<Utc>, DateTime<Utc>)>>,
	/// Tenant periods that a change stored after the candidates were read.
	changed: Option<RetentionRules>,
}

impl Snapshots {
	fn new(mut all: Vec<RetainedSnapshot>) -> Self {
		all.sort_by_key(|snapshot| snapshot.id);
		Self {
			all,
			purged: Mutex::default(),
			asked: Mutex::default(),
			changed: None,
		}
	}
}

#[async_trait]
impl SnapshotRetention for Snapshots {
	async fn candidates(
		&self,
		after: Option<Uuid>,
		retrieved_before: DateTime<Utc>,
		ended_before: DateTime<Utc>,
		limit: u32,
	) -> Result<Vec<RetainedSnapshot>> {
		self.asked
			.lock()
			.unwrap()
			.push((retrieved_before, ended_before));
		let purged = self.purged.lock().unwrap();
		Ok(self
			.all
			.iter()
			.filter(|snapshot| after.is_none_or(|after| snapshot.id > after))
			.filter(|snapshot| !purged.contains(&snapshot.id))
			.filter(|snapshot| {
				snapshot
					.run_ended_at
					.is_some_and(|ended| ended < ended_before)
					|| snapshot.fetched_at < retrieved_before
			})
			.take(limit as usize)
			.cloned()
			.collect())
	}
	async fn shortest_tenant_periods(&self) -> Result<Option<SnapshotPeriods>> {
		let periods = || {
			self.all
				.iter()
				.filter_map(|snapshot| snapshot.tenant_rules)
				.map(|rules| rules.document_snapshots)
		};
		Ok(periods()
			.map(|periods| periods.after_run_seconds)
			.min()
			.zip(periods().map(|periods| periods.max_age_seconds).min())
			.map(|(after_run_seconds, max_age_seconds)| SnapshotPeriods {
				after_run_seconds,
				max_age_seconds,
			}))
	}
	async fn purge(
		&self,
		ids: &[Uuid],
		expired: &(dyn for<'a> Fn(&'a RetainedSnapshot) -> bool + Sync),
		_: DateTime<Utc>,
	) -> Result<u64> {
		// Read again with the Tenant's current periods.
		let still = self
			.all
			.iter()
			.filter(|snapshot| ids.contains(&snapshot.id))
			.map(|snapshot| RetainedSnapshot {
				tenant_rules: self.changed.or(snapshot.tenant_rules),
				..snapshot.clone()
			})
			.filter(|snapshot| expired(snapshot))
			.map(|snapshot| snapshot.id)
			.collect::<Vec<_>>();
		self.purged.lock().unwrap().extend(&still);
		Ok(still.len() as u64)
	}
}

fn snapshot(
	id: u128,
	fetched_hours: i64,
	ended_hours: Option<i64>,
	tenant: Option<RetentionRules>,
) -> RetainedSnapshot {
	RetainedSnapshot {
		id: Uuid::from_u128(id),
		fetched_at: hours(fetched_hours),
		run_ended_at: ended_hours.map(hours),
		tenant_rules: tenant,
	}
}

#[tokio::test]
async fn the_sweeper_purges_only_contents_without_a_basis() {
	let deployment = rules(24, 7 * 24);
	let snapshots = Snapshots::new(vec![
		// The Run is still working after three days: kept under the cap.
		snapshot(1, 0, None, None),
		// The Run ended 25 hours ago: past the post-run period.
		snapshot(2, 0, Some(47), None),
		// The Run ended 2 hours ago: still within the post-run period.
		snapshot(3, 0, Some(70), None),
		// The Tenant keeps contents one hour after its Runs end.
		snapshot(4, 0, Some(70), Some(rules(1, 7 * 24))),
		// Past the Tenant's two-day cap although its Run never ended.
		snapshot(5, 0, None, Some(rules(24, 48))),
	]);
	let purged = sweep(&snapshots, &deployment, hours(72), 2).await.unwrap();
	assert_eq!(purged, 3);
	let mut ids = snapshots.purged.lock().unwrap().clone();
	ids.sort();
	assert_eq!(ids, [2, 4, 5].map(Uuid::from_u128));
	// Snapshots are examined only past the shortest cap, and those of ended
	// Runs only past the shortest post-run period.
	assert!(
		snapshots
			.asked
			.lock()
			.unwrap()
			.iter()
			.all(|before| *before == (hours(72 - 48), hours(72 - 1)))
	);
}

#[tokio::test]
async fn a_run_that_ended_within_every_post_run_period_is_not_examined() {
	let deployment = rules(24, 7 * 24);
	let snapshots = Snapshots::new(vec![snapshot(1, 0, Some(70), None)]);
	sweep(&snapshots, &deployment, hours(72), BATCH)
		.await
		.unwrap();
	let asked = snapshots.asked.lock().unwrap().clone();
	assert_eq!(asked, [(hours(72 - 7 * 24), hours(72 - 24))]);
	// The fake answers like the store: the Run that ended two hours ago is
	// not a candidate at all.
	assert!(snapshots.purged.lock().unwrap().is_empty());
}

#[tokio::test]
async fn periods_a_tenant_lengthened_after_the_candidates_were_read_still_apply() {
	// Read under a one-hour post-run period, purged after the Tenant cleared
	// it, so the Deployment's day applies again.
	let snapshots = Snapshots {
		changed: Some(rules(24, 7 * 24)),
		..Snapshots::new(vec![snapshot(1, 0, Some(70), Some(rules(1, 7 * 24)))])
	};
	let purged = sweep(&snapshots, &rules(24, 7 * 24), hours(72), BATCH)
		.await
		.unwrap();
	assert_eq!(purged, 0);
	assert!(snapshots.purged.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_deployment_that_shortened_its_periods_purges_what_it_already_kept() {
	// Stored by the Tenant before the Deployment cut its cap to one day.
	let snapshots = Snapshots::new(vec![snapshot(1, 0, None, Some(rules(24, 7 * 24)))]);
	let purged = sweep(&snapshots, &rules(24, 24), hours(25), BATCH)
		.await
		.unwrap();
	assert_eq!(purged, 1);
}

#[tokio::test]
async fn nothing_is_purged_while_every_basis_holds() {
	let snapshots = Snapshots::new(vec![snapshot(1, 0, None, None)]);
	let purged = sweep(&snapshots, &RetentionRules::DEFAULT, hours(1), BATCH)
		.await
		.unwrap();
	assert_eq!(purged, 0);
	assert!(snapshots.purged.lock().unwrap().is_empty());
}
