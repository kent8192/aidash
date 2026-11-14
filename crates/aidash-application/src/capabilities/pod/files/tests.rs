use super::*;
use crate::capabilities::pod::tests::{Runtime, context};
use crate::ports::capabilities::execution_pod::FileSearchResult;
use aidash_domain::capabilities::files::ScratchScope;
use std::sync::Arc;

fn scratch_read(path: &str, offset: Option<u64>, max_bytes: Option<usize>) -> ScratchRead {
	ScratchRead {
		path: path.into(),
		offset,
		max_bytes,
	}
}

#[tokio::test]
async fn a_read_stops_before_a_cut_character_and_continues_from_there() {
	let runtime = Arc::new(Runtime::default());
	runtime.file("notes/é.txt", "abé".as_bytes());
	let context = context(runtime.clone());
	let first = read(&context, scratch_read("notes/é.txt", None, Some(3)), 16)
		.await
		.unwrap();
	assert_eq!(first["content"], "ab");
	assert_eq!(first["next_offset"], 2);
	assert_eq!(first["truncated"], true);
	let rest = read(&context, scratch_read("notes/é.txt", Some(2), None), 16)
		.await
		.unwrap();
	assert_eq!(rest["content"], "é");
	assert_eq!(rest["truncated"], false);
	assert!(rest["next_offset"].is_null());
}

#[tokio::test]
async fn a_read_outside_the_scratch_space_is_refused_before_the_runner() {
	let runtime = Arc::new(Runtime::default());
	let context = context(runtime.clone());
	for path in ["../etc/passwd", "/etc/passwd", "a//b"] {
		assert!(matches!(
			read(&context, scratch_read(path, None, None), 16).await,
			Err(Error::Invalid(_)) | Err(Error::Domain(_))
		));
	}
	assert!(runtime.calls().is_empty());
}

#[tokio::test]
async fn a_stopped_pod_is_started_once_and_the_read_repeated() {
	let runtime = Arc::new(Runtime::default());
	runtime.file("a.txt", b"hello");
	*runtime.unavailable.lock().unwrap() = 1;
	let context = context(runtime.clone());
	let result = read(&context, scratch_read("a.txt", None, None), 16)
		.await
		.unwrap();
	assert_eq!(result["content"], "hello");
	assert_eq!(runtime.calls(), ["file_read", "ensure", "file_read"]);
}

#[tokio::test]
async fn a_line_search_is_bounded_and_keeps_the_runners_matches() {
	let runtime = Arc::new(Runtime::default());
	*runtime.search.lock().unwrap() = Some(FileSearchResult {
		matches: vec![crate::ports::capabilities::execution_pod::FileMatch {
			path: "src/main.rs".into(),
			line: 3,
			text: "fn main() {}".into(),
		}],
		truncated: true,
	});
	let context = context(runtime.clone());
	let result = search(
		&context,
		ScratchSearch {
			query: "fn main".into(),
			mode: SearchMode::Literal,
			scope: ScratchScope::Scratch,
			path: Some("src".into()),
			glob: Some("*.rs".into()),
			limit: Some(10_000),
		},
		20,
	)
	.await
	.unwrap();
	let request = runtime.searches.lock().unwrap()[0].clone();
	assert_eq!(request.max_results, 500);
	assert!(!request.regex);
	assert_eq!(request.path.as_deref(), Some("src"));
	assert_eq!(result["matches"][0]["path"], "src/main.rs");
	assert_eq!(result["matches"][0]["location"]["line"], 3);
	assert_eq!(result["truncated"], true);
}

#[tokio::test]
async fn a_path_search_lists_matching_paths_from_the_manifest() {
	let runtime = Arc::new(Runtime::default());
	runtime.file("src/main.rs", b"x");
	runtime.file("src/lib.rs", b"y");
	runtime.file("README.md", b"z");
	let context = context(runtime.clone());
	let result = search(
		&context,
		ScratchSearch {
			query: String::new(),
			mode: SearchMode::Path,
			scope: ScratchScope::Scratch,
			path: None,
			glob: Some("src/*.rs".into()),
			limit: Some(1),
		},
		20,
	)
	.await
	.unwrap();
	assert_eq!(result["matches"].as_array().unwrap().len(), 1);
	assert_eq!(result["truncated"], true);
	assert_eq!(
		runtime.manifests.lock().unwrap()[0].ignore_files,
		[".gitignore"]
	);
}

#[tokio::test]
async fn a_large_write_arrives_in_bounded_chunks_and_appears_only_at_the_end() {
	let runtime = Arc::new(Runtime::default());
	let context = context(runtime.clone());
	let bytes = vec![7_u8; CHUNK_BYTES + 5];
	let size = write(&context, "run", "out/data.bin", &bytes, None)
		.await
		.unwrap();
	assert_eq!(size, bytes.len() as u64);
	let writes = runtime.writes.lock().unwrap().clone();
	assert_eq!(writes.len(), 2);
	assert_eq!((writes[0].offset, writes[0].final_chunk), (0, false));
	assert_eq!(
		(writes[1].offset, writes[1].final_chunk),
		(CHUNK_BYTES as u64, true)
	);
	assert_eq!(writes[1].run_id, "run");
}

#[tokio::test]
async fn an_empty_file_is_written_with_one_final_chunk() {
	let runtime = Arc::new(Runtime::default());
	let context = context(runtime.clone());
	write(&context, "run", "empty.txt", b"", None)
		.await
		.unwrap();
	let writes = runtime.writes.lock().unwrap().clone();
	assert_eq!(writes.len(), 1);
	assert!(writes[0].final_chunk);
}

#[tokio::test]
async fn a_patch_reaches_the_runner_with_the_run() {
	let runtime = Arc::new(Runtime::default());
	let context = context(runtime.clone());
	let result = patch(&context, "run", "*** Begin Patch\n*** End Patch".into())
		.await
		.unwrap();
	assert_eq!(result["changed"], json!(["a.txt"]));
	assert_eq!(runtime.patches.lock().unwrap()[0].run_id, "run");
}
