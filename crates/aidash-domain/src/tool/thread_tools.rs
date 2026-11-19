//! The Thread tools (#1008, ADR 0140) and the Workspace tools they replace.
//!
//! `thread_read`, `thread_observe`, `thread_search`, `thread_message` and
//! `task_wait` are the default tools. The `workspace_*` tools they replace stay
//! declared with their original bytes, because saved Agent versions pin them;
//! both names reach the same implementation.

/// Each Thread tool and the Workspace tool it replaces.
pub const RENAMED: [(&str, &str); 5] = [
	("thread_read", "workspace_read"),
	("thread_observe", "workspace_observe"),
	("thread_search", "workspace_search"),
	("thread_message", "workspace_message"),
	("task_wait", "workspace_wait"),
];

/// The Workspace tools, still declared for pinned Agent versions.
pub const WORKSPACE_TOOLS: [&str; 5] = [
	"workspace_read",
	"workspace_observe",
	"workspace_search",
	"workspace_message",
	"workspace_wait",
];

/// The current name of a builtin operation: a Workspace tool's Thread tool,
/// any other operation unchanged.
pub fn current(operation: &str) -> &str {
	RENAMED
		.iter()
		.find(|(_, former)| *former == operation)
		.map_or(operation, |(current, _)| current)
}

/// The Workspace tool a Thread tool replaces, if `operation` is one.
pub fn former(operation: &str) -> Option<&'static str> {
	RENAMED
		.iter()
		.find(|(current, _)| *current == operation)
		.map(|(_, former)| *former)
}

/// Whether `name` is `operation` under its current or its former name.
pub fn is(name: &str, operation: &str) -> bool {
	current(name) == current(operation)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn both_names_reach_the_same_operation() {
		for (current_name, former_name) in RENAMED {
			assert_eq!(current(former_name), current_name);
			assert_eq!(current(current_name), current_name);
			assert_eq!(former(current_name), Some(former_name));
			assert!(is(former_name, current_name));
		}
		assert_eq!(current("task_create"), "task_create");
		assert_eq!(former("task_create"), None);
		assert!(!is("thread_read", "thread_message"));
	}
}
