//! Process resource sampling for Egress fair share.
use std::time::Duration;

pub const RECOMMENDED_OPEN_FILES: u64 = 65_536;
pub(super) const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

pub fn open_file_limits() -> Option<(u64, u64)> {
	#[cfg(unix)]
	{
		let mut limit = libc::rlimit {
			rlim_cur: 0,
			rlim_max: 0,
		};
		// SAFETY: getrlimit writes to an initialized, valid rlimit.
		if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
			return Some((limit.rlim_cur, limit.rlim_max));
		}
	}
	None
}

pub(super) fn sample(manager: &super::proxy::TunnelManager, memory_limit: Option<u64>) {
	#[cfg(target_os = "linux")]
	const FD_PATH: &str = "/proc/self/fd";
	#[cfg(not(target_os = "linux"))]
	const FD_PATH: &str = "/dev/fd";
	let files = std::fs::read_dir(FD_PATH).ok().map(|entries| {
		// The directory iteration adds one descriptor of its own.
		entries.count().saturating_sub(1)
	});
	let rss = rss_bytes();
	if let Some(files) = files {
		metrics::gauge!("aidash_egress_process_open_files").set(files as f64);
	}
	if let Some(rss) = rss {
		metrics::gauge!("aidash_egress_process_rss_bytes").set(rss as f64);
	}
	manager.buffers.expire_unused();
	metrics::gauge!("aidash_egress_borrowed_buffer_bytes")
		.set(manager.buffers.borrowed_bytes() as f64);
	let files = files
		.zip(open_file_limits())
		.map(|(used, (limit, _))| used as f64 / limit as f64);
	manager.update_pressure(files, fraction(rss, memory_limit));
}

fn fraction(rss: Option<u64>, limit: Option<u64>) -> Option<f64> {
	rss.zip(limit)
		.map(|(used, limit)| used as f64 / limit as f64)
}

pub(super) fn under_pressure(previous: bool, fraction: Option<f64>) -> bool {
	fraction.map_or(previous, |fraction| {
		if previous {
			fraction >= 0.70
		} else {
			fraction > 0.80
		}
	})
}

/// The API runs separately from the Egress gateway. Sample its own process with the same
/// memory signal and hysteresis the gateway uses (ADR 0150).
pub struct ReplicaMemoryPressure {
	limit: Option<u64>,
	short: std::sync::atomic::AtomicBool,
}

impl Default for ReplicaMemoryPressure {
	fn default() -> Self {
		Self {
			limit: memory_limit(None),
			short: std::sync::atomic::AtomicBool::new(false),
		}
	}
}

impl ReplicaMemoryPressure {
	pub fn sample(&self) {
		use std::sync::atomic::Ordering::Relaxed;
		let previous = self.short.load(Relaxed);
		self.short.store(
			under_pressure(previous, fraction(rss_bytes(), self.limit)),
			Relaxed,
		);
	}

	pub fn is_short(&self) -> bool {
		self.short.load(std::sync::atomic::Ordering::Relaxed)
	}
}

fn rss_bytes() -> Option<u64> {
	#[cfg(target_os = "macos")]
	{
		let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
		let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
		// SAFETY: proc_pidinfo initializes the task info if it returns its full size.
		let bytes = unsafe {
			libc::proc_pidinfo(
				libc::getpid(),
				libc::PROC_PIDTASKINFO,
				0,
				info.as_mut_ptr().cast(),
				size,
			)
		};
		if bytes == size {
			return Some(unsafe { info.assume_init() }.pti_resident_size);
		}
	}
	#[cfg(target_os = "linux")]
	{
		let status = std::fs::read_to_string("/proc/self/status").ok()?;
		let rss = status
			.lines()
			.find_map(|line| line.strip_prefix("VmRSS:"))?;
		Some(rss.split_whitespace().next()?.parse::<u64>().ok()? * 1024)
	}
	#[cfg(not(target_os = "linux"))]
	{
		None
	}
}

pub(super) fn memory_limit(configured: Option<u64>) -> Option<u64> {
	configured.or_else(|| {
		[
			"/sys/fs/cgroup/memory.max",
			"/sys/fs/cgroup/memory/memory.limit_in_bytes",
		]
		.into_iter()
		.find_map(|path| std::fs::read_to_string(path).ok()?.trim().parse().ok())
	})
}
