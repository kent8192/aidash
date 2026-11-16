//! Run-keyed Journal Query session admission.

use crate::{
	Error, Result,
	ports::capabilities::journal_query::{Authority, JournalSessions, Session},
};
use uuid::Uuid;

pub mod runner;

pub enum OpenSession {
	Ready { session_id: Uuid },
	Reset { session_id: Uuid },
}

pub fn fingerprint(authority: &Authority) -> String {
	aidash_domain::registry::rules::digest(&serde_json::json!(authority))
}

pub async fn open_session(
	scope: &mut dyn JournalSessions,
	authority: &Authority,
) -> Result<OpenSession> {
	let current = fingerprint(authority);
	let old = scope.load(authority.run).await?;
	if let Some(session) = &old {
		if session.fingerprint == current {
			return Ok(OpenSession::Ready {
				session_id: session.id,
			});
		}
		if !scope.stop(session.id).await? {
			return Err(Error::Conflict("JOURNAL_TERMINATION_UNCONFIRMED".into()));
		}
	}
	let session = Session {
		id: Uuid::new_v4(),
		fingerprint: current,
	};
	scope.save(authority.run, &session).await?;
	Ok(if old.is_some() {
		OpenSession::Reset {
			session_id: session.id,
		}
	} else {
		OpenSession::Ready {
			session_id: session.id,
		}
	})
}

#[cfg(test)]
mod tests;
