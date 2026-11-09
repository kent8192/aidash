//! The process's limit on open file descriptors. Every listener, connection and
//! database connection of the process draws on it, so a shortage in one of them
//! starves the others (#780). Only the soft limit is raised; the hard limit is
//! what the operator or container granted.

/// Raises the soft limit to the hard one and returns the soft limit then in
/// force. macOS refuses a soft limit above `OPEN_MAX` even under an unlimited
/// hard limit, so there it falls back to that.
#[cfg(unix)]
pub fn raise_soft_limit() -> std::io::Result<libc::rlim_t> {
	let mut limit = libc::rlimit {
		rlim_cur: 0,
		rlim_max: 0,
	};
	// SAFETY: a plain system call writing into a valid `rlimit`.
	if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
		return Err(std::io::Error::last_os_error());
	}
	if limit.rlim_cur >= limit.rlim_max {
		return Ok(limit.rlim_cur);
	}
	let wanted = limit.rlim_max;
	match set_soft(limit, wanted) {
		Ok(()) => Ok(wanted),
		#[cfg(target_os = "macos")]
		Err(_) if wanted > OPEN_MAX && limit.rlim_cur < OPEN_MAX => {
			set_soft(limit, OPEN_MAX)?;
			Ok(OPEN_MAX)
		}
		Err(error) => Err(error),
	}
}

/// `OPEN_MAX` from `<sys/syslimits.h>`.
#[cfg(target_os = "macos")]
const OPEN_MAX: libc::rlim_t = 10240;

#[cfg(unix)]
fn set_soft(mut limit: libc::rlimit, soft: libc::rlim_t) -> std::io::Result<()> {
	limit.rlim_cur = soft;
	// SAFETY: a plain system call with a pointer to a valid `rlimit`.
	if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } == 0 {
		Ok(())
	} else {
		Err(std::io::Error::last_os_error())
	}
}

#[cfg(test)]
mod tests {
	#[cfg(unix)]
	#[test]
	fn the_soft_limit_never_goes_down() {
		let mut before = libc::rlimit {
			rlim_cur: 0,
			rlim_max: 0,
		};
		// SAFETY: as above.
		assert_eq!(
			unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut before) },
			0
		);
		let raised = super::raise_soft_limit().unwrap();
		assert!(raised >= before.rlim_cur);
		assert!(raised <= before.rlim_max);
	}
}
