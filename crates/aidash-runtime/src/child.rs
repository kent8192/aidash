//! One-shot child processes under operating-system limits: one request on
//! standard input, one bounded reply on standard output. The child inherits no
//! environment, so it never holds a Deployment secret, and it is killed when
//! the exchange ends early.
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The limits one child runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
	/// CPU time after which the kernel stops the child.
	pub cpu_seconds: u64,
	/// Required hard address-space ceiling, or `None` when the caller does not
	/// need memory containment.
	pub memory_bytes: Option<u64>,
	/// Wall-clock deadline of the whole exchange.
	pub deadline: Duration,
	/// The largest reply accepted on standard output.
	pub max_output: usize,
}

/// Why a child produced no reply.
#[derive(Debug)]
pub enum Failure {
	/// The child could not be started or its pipes failed.
	Io(std::io::Error),
	/// The deadline passed; the child was killed.
	TimedOut,
	/// The reply exceeded `max_output`; the child was killed.
	OutputTooLarge,
	/// The child ended by a signal, as when it exceeds its CPU or memory limit.
	Killed,
	/// The child exited with a failure status.
	Exited(i32),
}

/// Run `program` once with `input` on its standard input and return what it
/// wrote to standard output after exiting successfully.
pub async fn run(
	program: &Path,
	args: &[&str],
	input: &[u8],
	limits: Limits,
) -> Result<Vec<u8>, Failure> {
	#[cfg(not(target_os = "linux"))]
	if limits.memory_bytes.is_some() {
		return Err(Failure::Io(std::io::Error::new(
			std::io::ErrorKind::Unsupported,
			"hard child address-space limits are unavailable on this platform",
		)));
	}
	let mut command = tokio::process::Command::new(program);
	command
		.args(args)
		.env_clear()
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.kill_on_drop(true);
	#[cfg(unix)]
	{
		// SAFETY: the closure runs in the forked child before exec and only
		// calls setrlimit, which is async-signal-safe.
		unsafe {
			command.pre_exec(move || {
				set_limit(libc::RLIMIT_CPU, limits.cpu_seconds)?;
				set_limit(libc::RLIMIT_CORE, 0)?;
				#[cfg(target_os = "linux")]
				if let Some(memory_bytes) = limits.memory_bytes {
					set_limit(libc::RLIMIT_AS, memory_bytes)?;
				}
				Ok(())
			});
		}
	}
	let mut child = command.spawn().map_err(Failure::Io)?;
	let mut stdin = child
		.stdin
		.take()
		.ok_or_else(|| Failure::Io(std::io::Error::other("the child has no standard input")))?;
	let mut stdout = child
		.stdout
		.take()
		.ok_or_else(|| Failure::Io(std::io::Error::other("the child has no standard output")))?;
	let exchange = async {
		// Writing and reading run together, so a child that answers before it
		// has read all of its input cannot block the exchange.
		let write = async {
			// A child may exit without reading everything; its status decides.
			let _ = stdin.write_all(input).await;
			drop(stdin);
		};
		let read = async {
			let mut output = Vec::new();
			(&mut stdout)
				.take(limits.max_output as u64 + 1)
				.read_to_end(&mut output)
				.await
				.map(|_| output)
		};
		let ((), output) = tokio::join!(write, read);
		let output = output.map_err(Failure::Io)?;
		if output.len() > limits.max_output {
			return Err(Failure::OutputTooLarge);
		}
		let status = child.wait().await.map_err(Failure::Io)?;
		match status.code() {
			Some(0) => Ok(output),
			Some(code) => Err(Failure::Exited(code)),
			None => Err(Failure::Killed),
		}
	};
	let result = tokio::time::timeout(limits.deadline, exchange)
		.await
		.unwrap_or(Err(Failure::TimedOut));
	if result.is_err() {
		// Already exited children ignore the kill; the others stop now.
		let _ = child.start_kill();
		let _ = child.wait().await;
	}
	result
}

#[cfg(all(unix, target_os = "linux", target_env = "gnu"))]
type Resource = libc::__rlimit_resource_t;
#[cfg(all(unix, not(all(target_os = "linux", target_env = "gnu"))))]
type Resource = libc::c_int;

#[cfg(unix)]
fn set_limit(resource: Resource, value: u64) -> std::io::Result<()> {
	let limit = libc::rlimit {
		rlim_cur: value as libc::rlim_t,
		rlim_max: value as libc::rlim_t,
	};
	// SAFETY: a plain system call with a pointer to a valid `rlimit`.
	if unsafe { libc::setrlimit(resource, &limit) } == 0 {
		Ok(())
	} else {
		Err(std::io::Error::last_os_error())
	}
}

#[cfg(test)]
mod tests;
