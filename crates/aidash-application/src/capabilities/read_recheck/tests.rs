//! The schedule decides when to check and what each outcome does to a target.
use super::*;
use crate::Error;
use async_trait::async_trait;
use rstest::rstest;
use std::sync::Mutex;

#[derive(Default)]
struct Repository {
	targets: Vec<Target>,
	outcomes: Mutex<BTreeMap<Target, Outcome>>,
	checks: Mutex<Vec<Target>>,
	refused: Mutex<Vec<Target>>,
	unconfirmed: Mutex<Vec<Uuid>>,
	stop_fails: bool,
	check_fails: bool,
}

impl Repository {
	fn with(target: Target, outcome: Outcome) -> Self {
		Self {
			targets: vec![target],
			outcomes: Mutex::new([(target, outcome)].into()),
			..Default::default()
		}
	}
	fn checks(&self) -> usize {
		self.checks.lock().unwrap().len()
	}
}

#[async_trait]
impl ReadRecheckRepository for Repository {
	async fn targets(&self) -> Result<Vec<Target>> {
		Ok(self.targets.clone())
	}
	async fn check(&self, target: Target) -> Result<Outcome> {
		self.checks.lock().unwrap().push(target);
		if self.check_fails {
			return Err(Error::External("database unavailable".into()));
		}
		Ok(self
			.outcomes
			.lock()
			.unwrap()
			.get(&target)
			.copied()
			.unwrap_or(Outcome::Allowed))
	}
	async fn stop_refused(&self, target: Target) -> Result<()> {
		if self.stop_fails {
			return Err(Error::External("runner unavailable".into()));
		}
		self.refused.lock().unwrap().push(target);
		Ok(())
	}
	async fn stop_unconfirmed(&self, preview: Uuid) -> Result<()> {
		self.unconfirmed.lock().unwrap().push(preview);
		Ok(())
	}
}

fn operation() -> Target {
	Target::Operation(Uuid::from_u128(1))
}
fn preview() -> Target {
	Target::LivePreview(Uuid::from_u128(2))
}

#[tokio::test]
async fn a_target_is_checked_at_once_and_then_every_period() {
	// Arrange
	let repository = Repository::with(operation(), Outcome::Allowed);
	let mut schedule = Schedule::default();
	let start = Instant::now();

	// Act and Assert: first sight checks, including after a restart.
	pass(&repository, &mut schedule, start).await.unwrap();
	assert_eq!(repository.checks(), 1);
	pass(&repository, &mut schedule, start + Duration::from_secs(29))
		.await
		.unwrap();
	assert_eq!(repository.checks(), 1, "not due before the period ends");
	pass(&repository, &mut schedule, start + PERIOD)
		.await
		.unwrap();
	assert_eq!(repository.checks(), 2);
}

#[rstest]
#[case::command(operation())]
#[case::live_preview(preview())]
#[tokio::test]
async fn a_refusal_stops_the_target_as_a_revocation(#[case] target: Target) {
	// Arrange
	let repository = Repository::with(target, Outcome::Refused);

	// Act
	pass(&repository, &mut Schedule::default(), Instant::now())
		.await
		.unwrap();

	// Assert
	assert_eq!(*repository.refused.lock().unwrap(), vec![target]);
}

#[tokio::test]
async fn a_failed_stop_is_retried_after_a_growing_wait_that_never_passes_a_period() {
	// Arrange
	let repository = Repository {
		stop_fails: true,
		..Repository::with(operation(), Outcome::Refused)
	};
	let mut schedule = Schedule::default();
	let start = Instant::now();

	// Act and Assert: the first failure waits 5 seconds, the next 10, then 20, and
	// no wait is longer than a period.
	for (seconds, retried) in [
		(0, true),
		(1, false),
		(4, false),
		(5, true),
		(14, false),
		(15, true),
		(34, false),
		(35, true),
		(64, false),
		(65, true),
		(94, false),
		(95, true),
	] {
		let before = repository.checks();
		let now = start + Duration::from_secs(seconds);
		pass(&repository, &mut schedule, now).await.unwrap();
		assert_eq!(repository.checks() > before, retried, "at {seconds} s");
	}
}

#[tokio::test]
async fn a_stop_that_works_ends_the_retries() {
	// Arrange
	let target = operation();
	let mut repository = Repository {
		stop_fails: true,
		..Repository::with(target, Outcome::Refused)
	};
	let mut schedule = Schedule::default();
	let start = Instant::now();
	pass(&repository, &mut schedule, start).await.unwrap();

	// Act
	repository.stop_fails = false;
	pass(&repository, &mut schedule, start + RETRY_FROM)
		.await
		.unwrap();
	pass(
		&repository,
		&mut schedule,
		start + RETRY_FROM + Duration::from_secs(1),
	)
	.await
	.unwrap();

	// Assert: one stop, and then the ordinary period.
	assert_eq!(*repository.refused.lock().unwrap(), vec![target]);
	assert_eq!(repository.checks(), 2);
}

#[tokio::test]
async fn an_unanswered_check_leaves_a_command_running() {
	// Arrange
	let repository = Repository::with(operation(), Outcome::Unconfirmed);
	let mut schedule = Schedule::default();
	let start = Instant::now();

	// Act
	for round in 0..20 {
		pass(&repository, &mut schedule, start + PERIOD * round)
			.await
			.unwrap();
	}

	// Assert
	assert_eq!(repository.checks(), 20);
	assert!(repository.refused.lock().unwrap().is_empty());
	assert!(repository.unconfirmed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_live_preview_stops_after_five_unanswered_checks_in_a_row() {
	// Arrange
	let target = preview();
	let Target::LivePreview(id) = target else {
		unreachable!()
	};
	let repository = Repository::with(target, Outcome::Unconfirmed);
	let mut schedule = Schedule::default();
	let start = Instant::now();

	// Act and Assert: four misses keep it serving, and the fifth stops it.
	for round in 0..4 {
		pass(&repository, &mut schedule, start + PERIOD * round)
			.await
			.unwrap();
	}
	assert!(repository.unconfirmed.lock().unwrap().is_empty());
	pass(&repository, &mut schedule, start + PERIOD * 4)
		.await
		.unwrap();
	assert_eq!(*repository.unconfirmed.lock().unwrap(), vec![id]);
	assert!(
		repository.refused.lock().unwrap().is_empty(),
		"an unanswered check is not a revocation"
	);
}

#[tokio::test]
async fn an_answer_resets_the_unanswered_count() {
	// Arrange
	let target = preview();
	let repository = Repository::with(target, Outcome::Unconfirmed);
	let mut schedule = Schedule::default();
	let start = Instant::now();
	for round in 0..4 {
		pass(&repository, &mut schedule, start + PERIOD * round)
			.await
			.unwrap();
	}

	// Act: one answer, then four more misses.
	repository
		.outcomes
		.lock()
		.unwrap()
		.insert(target, Outcome::Allowed);
	pass(&repository, &mut schedule, start + PERIOD * 4)
		.await
		.unwrap();
	repository
		.outcomes
		.lock()
		.unwrap()
		.insert(target, Outcome::Unconfirmed);
	for round in 5..9 {
		pass(&repository, &mut schedule, start + PERIOD * round)
			.await
			.unwrap();
	}

	// Assert
	assert!(repository.unconfirmed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_check_decides_nothing_and_does_not_block_the_next_target() {
	// Arrange
	let repository = Repository {
		targets: vec![operation(), preview()],
		check_fails: true,
		..Default::default()
	};

	// Act
	pass(&repository, &mut Schedule::default(), Instant::now())
		.await
		.unwrap();

	// Assert
	assert_eq!(repository.checks(), 2);
	assert!(repository.refused.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_ended_target_is_forgotten() {
	// Arrange
	let target = preview();
	let mut schedule = Schedule::default();
	let start = Instant::now();
	pass(
		&Repository::with(target, Outcome::Unconfirmed),
		&mut schedule,
		start,
	)
	.await
	.unwrap();

	// Act
	pass(&Repository::default(), &mut schedule, start)
		.await
		.unwrap();

	// Assert
	assert!(schedule.checked.is_empty());
	assert!(schedule.unconfirmed.is_empty());
}
