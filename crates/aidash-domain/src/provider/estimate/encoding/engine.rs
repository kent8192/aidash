//! The embedded tokenizer files and the counting engine behind them. Each
//! file is decompressed and checked against its pinned digest on first use;
//! a file that does not match leaves its encoding unavailable.
use super::{CHUNK_BYTES, TokenEncoding, chunks};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::{LazyLock, Mutex, OnceLock};

/// The gpt-oss `tokenizer.json`, revision b5c939de8f754692c1647ca79fbf85e8c1e70f8a.
const O200K_BASE: &[u8] = include_bytes!("../../../../assets/token-encodings/o200k_base.json.zst");
/// SHA-256 of the decompressed gpt-oss `tokenizer.json`.
pub(super) const O200K_BASE_SHA256: &str =
	"0614fe83cadab421296e664e1f48f4261fa8fef6e03e63bb75c20f38e37d07d3";
/// The Gemma 4 E4B `tokenizer.json`, revision ee0ef6023621cff504d758262d4e04895a5af4a2.
const GEMMA4: &[u8] = include_bytes!("../../../../assets/token-encodings/gemma4.json.zst");
/// SHA-256 of the decompressed Gemma 4 `tokenizer.json`.
pub(super) const GEMMA4_SHA256: &str =
	"cc8d3a0ce36466ccc1278bf987df5f71db1719b9ca6b4118264f45cb627bfe0f";

/// Counted chunks kept for reuse across the requests of a step and of
/// concurrent Runs. Only counts are kept, never text.
const CACHE_ENTRIES: usize = 1 << 16;

pub(super) struct Engine {
	encoding: TokenEncoding,
	tokenizer: tokenizers::Tokenizer,
}

static ENGINES: [OnceLock<Option<Engine>>; TokenEncoding::ALL.len()] =
	[const { OnceLock::new() }; TokenEncoding::ALL.len()];
static COUNTS: LazyLock<Mutex<HashMap<(TokenEncoding, u64), u32>>> =
	LazyLock::new(|| Mutex::new(HashMap::new()));

impl Engine {
	pub(super) fn load(encoding: TokenEncoding) -> Option<&'static Self> {
		ENGINES[encoding as usize]
			.get_or_init(|| Self::build(encoding))
			.as_ref()
	}

	fn build(encoding: TokenEncoding) -> Option<Self> {
		let (compressed, digest) = match encoding {
			TokenEncoding::O200kBase => (O200K_BASE, O200K_BASE_SHA256),
			TokenEncoding::Gemma4 => (GEMMA4, GEMMA4_SHA256),
		};
		let file = decompress(compressed)?;
		if format!("{:x}", Sha256::digest(&file)) != digest {
			return None;
		}
		let mut tokenizer = tokenizers::Tokenizer::from_bytes(&file).ok()?;
		// Special-token text in a message is ordinary text to the provider,
		// so it is counted like any other text.
		tokenizer.set_encode_special_tokens(true);
		Some(Self {
			encoding,
			tokenizer,
		})
	}

	pub(super) fn count(&self, text: &str) -> usize {
		chunks(text, CHUNK_BYTES).fold(0_usize, |sum, chunk| {
			sum.saturating_add(self.count_chunk(chunk))
		})
	}

	fn count_chunk(&self, chunk: &str) -> usize {
		let mut hasher = std::hash::DefaultHasher::new();
		chunk.hash(&mut hasher);
		let key = (self.encoding, hasher.finish());
		if let Some(count) = COUNTS
			.lock()
			.ok()
			.and_then(|counts| counts.get(&key).copied())
		{
			return count as usize;
		}
		// A chunk the tokenizer rejects counts one token per byte, which no
		// encoding exceeds.
		let count = self
			.tokenizer
			.encode_fast(chunk, false)
			.map_or(chunk.len(), |encoding| encoding.len());
		if let Ok(mut counts) = COUNTS.lock() {
			if counts.len() >= CACHE_ENTRIES {
				counts.clear();
			}
			counts.insert(key, u32::try_from(count).unwrap_or(u32::MAX));
		}
		count
	}
}

fn decompress(compressed: &[u8]) -> Option<Vec<u8>> {
	let mut decoder = ruzstd::decoding::StreamingDecoder::new(compressed).ok()?;
	let mut file = Vec::new();
	decoder.read_to_end(&mut file).ok()?;
	Some(file)
}

/// Count without the shared cache, for benchmarks and reference checks.
#[cfg(test)]
pub(super) fn count_uncached(encoding: TokenEncoding, text: &str) -> Option<usize> {
	let engine = Engine::load(encoding)?;
	Some(
		engine
			.tokenizer
			.encode_fast(text, false)
			.map_or(text.len(), |encoding| encoding.len()),
	)
}
