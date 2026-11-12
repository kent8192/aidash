//! A per-call note of why an authority check refused, read when the call ends.
//!
//! `Error::Forbidden` is matched by hundreds of arms that treat it as "skip" or "not visible", so
//! a check cannot change what it returns. It leaves this note instead, and the harness reads it
//! once the tool call or worker step that raised the refusal has ended. A task spawned inside the
//! scope does not see it; a refusal raised there is an unclassified one.
//!
//! Outside a scope every function here does nothing, so HTTP paths are unaffected.
use aidash_domain::tool::RefusalCheck;
use std::{cell::RefCell, future::Future};

/// Why the last check refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
	pub check: RefusalCheck,
	/// The policy or Catalog action that was refused.
	pub action: Option<String>,
	/// The denied subject, when it lies outside the Run's own subject chain.
	pub subject: Option<String>,
}

impl Note {
	pub fn new(check: RefusalCheck, action: Option<&str>) -> Self {
		Self {
			check,
			action: action.map(str::to_owned),
			subject: None,
		}
	}
	pub fn subject(mut self, subject: Option<String>) -> Self {
		self.subject = subject;
		self
	}
}

tokio::task_local! {
	// Task-local values are immutable per scope, hence the cell.
	static NOTE: RefCell<Option<Note>>;
}

/// Record a refusal; the last one wins.
pub fn set(note: Note) {
	let _ = NOTE.try_with(|cell| *cell.borrow_mut() = Some(note));
}

/// An allowed policy check supersedes an earlier policy refusal in the same scope. Other notes
/// stay: a Catalog or Tenant Deletion refusal is not lifted by a later policy allow.
pub fn clear_policy() {
	let _ = NOTE.try_with(|cell| {
		let mut note = cell.borrow_mut();
		if note
			.as_ref()
			.is_some_and(|note| note.check == RefusalCheck::Policy)
		{
			*note = None;
		}
	});
}

/// Remove and return the note of the current scope, if there is a scope.
pub fn take() -> Option<Note> {
	NOTE.try_with(|cell| cell.borrow_mut().take())
		.ok()
		.flatten()
}

/// Run `work` in a scope of its own that inherits nothing, and return its output with the note
/// it left. Concurrent calls cannot see each other's notes.
pub async fn scoped<F: Future>(work: F) -> (F::Output, Option<Note>) {
	NOTE.scope(RefCell::new(None), async {
		let output = work.await;
		(output, take())
	})
	.await
}

/// Run `work` in a scope of its own and leave the note in it for [`take`] calls made by `work`.
pub async fn within<F: Future>(work: F) -> F::Output {
	NOTE.scope(RefCell::new(None), work).await
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn outside_a_scope_nothing_is_recorded() {
		set(Note::new(RefusalCheck::Policy, Some("task.read")));
		assert_eq!(take(), None);
	}

	#[tokio::test]
	async fn the_last_refusal_wins_and_an_allow_clears_a_policy_note() {
		let (_, note) = scoped(async {
			set(Note::new(RefusalCheck::Policy, Some("a")));
			set(Note::new(RefusalCheck::Policy, Some("b")));
		})
		.await;
		assert_eq!(note, Some(Note::new(RefusalCheck::Policy, Some("b"))));
		let (_, note) = scoped(async {
			set(Note::new(RefusalCheck::Policy, Some("a")));
			clear_policy();
		})
		.await;
		assert_eq!(note, None);
		let (_, note) = scoped(async {
			set(Note::new(RefusalCheck::Catalog, Some("a")));
			clear_policy();
		})
		.await;
		assert_eq!(note, Some(Note::new(RefusalCheck::Catalog, Some("a"))));
	}

	#[tokio::test]
	async fn concurrent_scopes_do_not_see_each_other() {
		let first = scoped(async {
			set(Note::new(RefusalCheck::Policy, Some("first")));
			tokio::task::yield_now().await;
		});
		let second = scoped(async {
			tokio::task::yield_now().await;
		});
		let ((_, first), (_, second)) = tokio::join!(first, second);
		assert_eq!(first, Some(Note::new(RefusalCheck::Policy, Some("first"))));
		assert_eq!(second, None);
	}

	#[tokio::test]
	async fn a_spawned_task_does_not_inherit_the_note() {
		let (_, note) = scoped(async {
			tokio::spawn(async { set(Note::new(RefusalCheck::Policy, None)) })
				.await
				.unwrap();
		})
		.await;
		assert_eq!(note, None);
	}
}
