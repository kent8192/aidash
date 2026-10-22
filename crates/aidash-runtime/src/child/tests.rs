use super::*;
use rstest::rstest;
use std::path::Path;

fn limits() -> Limits {
	Limits {
		cpu_seconds: 5,
		memory_bytes: None,
		deadline: Duration::from_secs(10),
		max_output: 1024,
	}
}

#[cfg(not(target_os = "linux"))]
#[rstest]
#[tokio::test]
async fn a_requested_memory_limit_fails_closed() {
	let mut limits = limits();
	limits.memory_bytes = Some(512 * 1024 * 1024);
	let error = shell("printf unsafe", b"", limits).await.unwrap_err();
	assert!(
		matches!(error, Failure::Io(ref error) if error.kind() == std::io::ErrorKind::Unsupported),
		"{error:?}"
	);
}

async fn shell(script: &str, input: &[u8], limits: Limits) -> Result<Vec<u8>, Failure> {
	run(Path::new("/bin/sh"), &["-c", script], input, limits).await
}

#[rstest]
#[tokio::test]
async fn a_child_answers_its_input_on_standard_output() {
	assert_eq!(
		shell("cat", b"request", limits()).await.unwrap(),
		b"request"
	);
}

#[rstest]
#[tokio::test]
async fn a_child_inherits_no_environment() {
	// HOME is set for the test process; the child must not see it, or any
	// Deployment secret the parent holds.
	assert!(std::env::var_os("HOME").is_some());
	let output = shell("printf %s \"$HOME\"", b"", limits()).await.unwrap();
	assert!(output.is_empty(), "{}", String::from_utf8_lossy(&output));
}

#[rstest]
#[tokio::test]
async fn an_oversized_reply_is_refused() {
	let error = shell("yes", b"", limits()).await.unwrap_err();
	assert!(matches!(error, Failure::OutputTooLarge), "{error:?}");
}

#[rstest]
#[tokio::test]
async fn the_deadline_kills_a_stalled_child() {
	let mut limits = limits();
	limits.deadline = Duration::from_millis(300);
	let started = std::time::Instant::now();
	let error = shell("sleep 30", b"", limits).await.unwrap_err();
	assert!(matches!(error, Failure::TimedOut), "{error:?}");
	assert!(started.elapsed() < Duration::from_secs(10));
}

#[rstest]
#[tokio::test]
async fn the_cpu_limit_stops_a_busy_child() {
	let mut limits = limits();
	limits.cpu_seconds = 1;
	let error = shell("while :; do :; done", b"", limits).await.unwrap_err();
	assert!(matches!(error, Failure::Killed), "{error:?}");
}

#[rstest]
#[tokio::test]
async fn a_failure_status_is_reported() {
	let error = shell("exit 3", b"", limits()).await.unwrap_err();
	assert!(matches!(error, Failure::Exited(3)), "{error:?}");
}

#[rstest]
#[tokio::test]
async fn a_child_that_ignores_its_input_still_answers() {
	let input = vec![b'x'; 4 * 1024 * 1024];
	assert_eq!(shell("printf ok", &input, limits()).await.unwrap(), b"ok");
}
