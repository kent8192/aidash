use super::*;
use rstest::rstest;

fn key() -> ReadVisitKey {
	(
		"aidash://node".into(),
		Uuid::from_u128(1),
		"authority".into(),
	)
}

#[rstest]
fn repeated_visit_is_a_cycle_only_while_its_owned_guard_is_live() {
	let visits = ReadVisits::default();
	let guard = visits.enter(key()).unwrap();
	assert!(visits.enter(key()).is_none());
	drop(guard);
	let next = visits.enter(key()).unwrap();
	assert!(visits.enter(key()).is_none());
	drop(next);
}

#[rstest]
#[case("node")]
#[case("run")]
#[case("authority")]
fn node_resource_and_authority_are_all_part_of_the_cycle_identity(#[case] part: &str) {
	let visits = ReadVisits::default();
	let original = visits.enter(key()).unwrap();
	let mut distinct = key();
	match part {
		"node" => distinct.0 = "aidash://peer".into(),
		"run" => distinct.1 = Uuid::from_u128(2),
		"authority" => distinct.2 = "new authority".into(),
		_ => panic!("invalid case"),
	}
	let different = visits.enter(distinct.clone()).unwrap();
	assert!(visits.enter(key()).is_none());
	assert!(visits.enter(distinct).is_none());
	drop(different);
	drop(original);
}

#[rstest]
fn dropping_a_guard_from_before_refresh_cannot_remove_a_new_visit_of_the_same_key() {
	let visits = ReadVisits::default();
	let old = visits.enter(key()).unwrap();
	visits.clear();
	let current = visits.enter(key()).unwrap();
	drop(old);
	assert!(visits.enter(key()).is_none());
	drop(current);
	assert!(visits.enter(key()).is_some());
}

fn failing_read(visits: &ReadVisits) -> std::io::Result<()> {
	let _guard = visits.enter(key()).unwrap();
	Err(std::io::Error::other("read failed"))
}
#[rstest]
fn failed_reads_release_the_visit_before_the_next_attempt() {
	let visits = ReadVisits::default();
	let error = failing_read(&visits).unwrap_err();
	assert_eq!(error.to_string(), "read failed");
	assert!(visits.enter(key()).is_some());
}

#[rstest]
fn panic_unwinding_releases_the_visit_without_poisoning_the_cache() {
	let visits = ReadVisits::default();
	let outcome = std::panic::catch_unwind(|| {
		let _guard = visits.enter(key()).unwrap();
		panic!("adapter panic");
	});
	assert!(outcome.is_err());
	assert!(visits.enter(key()).is_some());
}

struct AbortTask(tokio::task::JoinHandle<()>);
impl Drop for AbortTask {
	fn drop(&mut self) {
		self.0.abort();
	}
}
#[rstest]
#[tokio::test]
async fn cancelled_read_tasks_release_the_visit_for_a_later_authorized_attempt() {
	let visits = Arc::new(ReadVisits::default());
	let child = visits.clone();
	let (ready, started) = tokio::sync::oneshot::channel();
	let mut task = AbortTask(tokio::spawn(async move {
		let _guard = child.enter(key()).unwrap();
		ready.send(()).unwrap();
		std::future::pending::<()>().await;
	}));
	started.await.unwrap();
	let was_active = visits.enter(key()).is_none();
	task.0.abort();
	let completion = (&mut task.0).await.unwrap_err();
	assert!(was_active);
	assert!(completion.is_cancelled());
	assert!(visits.enter(key()).is_some());
}

fn run(id: u128) -> ReadVisitKey {
	(
		"aidash://node".into(),
		Uuid::from_u128(id),
		"authority".into(),
	)
}

#[rstest]
fn idleness_follows_the_live_guards() {
	let visits = ReadVisits::default();
	assert!(visits.is_idle());
	let guard = visits.enter(key()).unwrap();
	assert!(!visits.is_idle());
	drop(guard);
	assert!(visits.is_idle());
}

#[rstest]
fn an_outermost_true_settles_every_true_reached_inside_it() {
	let mut verdicts = ReadVerdicts::default();
	// B was checked inside A, while A was still in progress.
	verdicts.record(run(2), true, false);
	assert_eq!(verdicts.get(&run(2)), Some(true));
	verdicts.record(run(1), true, true);
	assert_eq!(verdicts.get(&run(1)), Some(true));
	assert_eq!(verdicts.get(&run(2)), Some(true));
}

#[rstest]
fn a_true_resting_on_a_failed_ancestor_is_dropped_not_reused() {
	let mut verdicts = ReadVerdicts::default();
	// A reads B's output and B reads A's: B's check met A in progress and
	// assumed it, then A failed on a denied source of its own.
	verdicts.record(run(2), true, false);
	verdicts.record(run(1), false, true);
	assert_eq!(verdicts.get(&run(1)), Some(false));
	// B is walked again by the next check instead of answering a stale true.
	assert_eq!(verdicts.get(&run(2)), None);
}

#[rstest]
fn a_denied_read_is_final_wherever_it_is_found() {
	let mut verdicts = ReadVerdicts::default();
	verdicts.record(run(2), false, false);
	verdicts.record(run(1), false, true);
	assert_eq!(verdicts.get(&run(2)), Some(false));
	assert_eq!(verdicts.get(&run(1)), Some(false));
}

#[rstest]
fn an_abandoned_or_cleared_check_settles_nothing() {
	let mut verdicts = ReadVerdicts::default();
	verdicts.record(run(2), true, false);
	verdicts.abandon();
	assert_eq!(verdicts.get(&run(2)), None);
	verdicts.record(run(3), true, true);
	verdicts.record(run(4), false, true);
	verdicts.clear();
	assert_eq!(verdicts.get(&run(3)), None);
	assert_eq!(verdicts.get(&run(4)), None);
}
