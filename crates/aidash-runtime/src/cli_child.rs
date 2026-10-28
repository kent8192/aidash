//! Bounded, cancellable CLI exchanges. No inherited environment or shell.
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
	Start,
	Io,
	Timeout,
	OutputLimit,
	Exit,
}

pub struct Output {
	pub stdout: Vec<u8>,
	pub success: bool,
}

/// Killing the process group also closes inherited pipes held by descendants.
/// This is cleanup, not a sandbox: callers must disable the CLI's native tools.
struct Group(u32);
impl Drop for Group {
	fn drop(&mut self) {
		#[cfg(unix)]
		// SAFETY: the child created its own process group before exec.
		unsafe {
			libc::kill(-(self.0 as i32), libc::SIGKILL);
		}
	}
}

pub async fn exchange(
	program: &Path,
	args: &[String],
	env: &BTreeMap<String, String>,
	cwd: &Path,
	input: &[u8],
	deadline: Duration,
	output_limit: usize,
) -> Result<Output, Failure> {
	if !cfg!(unix) {
		return Err(Failure::Start);
	}
	let mut cmd = tokio::process::Command::new(program);
	cmd.args(args)
		.env_clear()
		.envs(env)
		.current_dir(cwd)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.kill_on_drop(true);
	#[cfg(unix)]
	cmd.process_group(0);
	let mut child = cmd.spawn().map_err(|_| Failure::Start)?;
	let group = Group(child.id().ok_or(Failure::Start)?);
	let mut stdin = child.stdin.take().ok_or(Failure::Io)?;
	let stdout = child.stdout.take().ok_or(Failure::Io)?;
	let result = tokio::time::timeout(deadline, async {
		let write = async {
			stdin.write_all(input).await.map_err(|_| Failure::Io)?;
			drop(stdin);
			Ok::<(), Failure>(())
		};
		let read = async {
			let mut bytes = Vec::new();
			stdout
				.take(output_limit as u64 + 1)
				.read_to_end(&mut bytes)
				.await
				.map_err(|_| Failure::Io)?;
			if bytes.len() > output_limit {
				return Err(Failure::OutputLimit);
			}
			Ok(bytes)
		};
		let ((), stdout) = tokio::try_join!(write, read)?;
		let success = child.wait().await.map_err(|_| Failure::Io)?.success();
		Ok(Output { stdout, success })
	})
	.await
	.unwrap_or(Err(Failure::Timeout));
	drop(group);
	let _ = child.wait().await;
	result
}

#[cfg(all(test, unix))]
mod tests {
	use super::*;
	#[tokio::test]
	async fn timeout_output_bound_and_exit_status() {
		let env = BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]);
		let cwd = std::env::temp_dir();
		let call = |script: &'static str, limit| {
			exchange(
				Path::new("/bin/sh"),
				&[],
				&env,
				&cwd,
				script.as_bytes(),
				Duration::from_millis(150),
				limit,
			)
		};
		assert!(matches!(
			call("sleep 30 & wait", 1024).await,
			Err(Failure::Timeout)
		));
		assert!(matches!(
			call("yes output", 32).await,
			Err(Failure::OutputLimit)
		));
		let result = call("printf success", 1024).await.unwrap();
		assert_eq!(result.stdout, b"success");
		assert!(result.success);
		let result = call("exit 4", 1024).await.unwrap();
		assert!(!result.success);
	}

	#[tokio::test]
	async fn cancellation_stops_descendants_before_their_effect() {
		struct Directory(std::path::PathBuf);
		impl Drop for Directory {
			fn drop(&mut self) {
				let _ = std::fs::remove_dir_all(&self.0);
			}
		}
		let directory = Directory(
			std::env::temp_dir().join(format!("aidash-cli-test-{}", uuid::Uuid::new_v4())),
		);
		std::fs::create_dir(&directory.0).unwrap();
		let cwd = directory.0.clone();
		let ready = cwd.join("ready");
		let effect = cwd.join("effect");
		let env = BTreeMap::from([
			("PATH".into(), "/usr/bin:/bin".into()),
			("READY".into(), ready.to_string_lossy().into_owned()),
			("EFFECT".into(), effect.to_string_lossy().into_owned()),
		]);
		let task = tokio::spawn(async move {
			exchange(
				Path::new("/bin/sh"),
				&[],
				&env,
				&cwd,
				b"(sleep 0.6; printf escaped > \"$EFFECT\") & printf ready > \"$READY\"; wait",
				Duration::from_secs(3),
				1024,
			)
			.await
		});
		tokio::time::timeout(Duration::from_secs(2), async {
			while !ready.exists() {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
		task.abort();
		assert!(matches!(task.await,Err(error) if error.is_cancelled()));
		tokio::time::sleep(Duration::from_millis(800)).await;
		assert!(!effect.exists(), "a descendant survived cancellation");
	}
}
