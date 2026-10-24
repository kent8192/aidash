//! Scope environment-backed test credentials to one child test process.
use std::{sync::LazyLock, time::Duration};
use tokio::{process::Command, sync::Semaphore};

/// Return true only inside the selected child. The parent verifies the complete
/// child test result before returning false; it never mutates its environment.
pub async fn isolated_process(environment: &[(&str, &str)]) -> bool {
	let name = std::thread::current()
		.name()
		.expect("named libtest thread")
		.to_owned();
	if std::env::var("AIDASH_TEST_CHILD").as_deref() == Ok(name.as_str()) {
		for (key, value) in environment {
			assert_eq!(std::env::var(key).as_deref(), Ok(*value));
		}
		return true;
	}
	static CAPACITY: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(2));
	let _capacity = CAPACITY.acquire().await.unwrap();
	let child = Command::new(std::env::current_exe().unwrap())
		.args(["--exact", &name, "--nocapture"])
		.env("AIDASH_TEST_CHILD", &name)
		.envs(environment.iter().copied())
		.env("RUST_BACKTRACE", "0")
		.kill_on_drop(true)
		.output();
	let output = tokio::time::timeout(Duration::from_secs(180), child)
		.await
		.expect("isolated test deadline")
		.unwrap();
	assert!(
		output.status.success(),
		"{name}: {}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(
		String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed; 0 ignored"),
		"isolated process must execute exactly the selected test: {}",
		String::from_utf8_lossy(&output.stdout)
	);
	false
}
