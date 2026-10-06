//! Fsynced, independently retained CBOR fences and a persistent restore serving gate.
use crate::{Error, Result};
use aidash_application::ports::memory::MemoryRecovery;
use aidash_domain::memory::{Unit, recovery::Ledger};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
	fs::{File, OpenOptions},
	io::{Read, Write},
	path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"AIDMEM01";
pub(crate) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) struct FileRecovery {
	pub directory: PathBuf,
	pub home: String,
}
impl FileRecovery {
	pub fn new(directory: PathBuf, home: String) -> Result<Self> {
		if !directory.is_absolute() || home.trim().is_empty() {
			return Err(Error::Invalid(
				"memory recovery needs an absolute external directory and Home identity".into(),
			));
		}
		Ok(Self { directory, home })
	}
	fn lock(&self) -> Result<File> {
		// Do not create directories during serving. Initialization is explicit;
		// a missing external volume/ledger is a closed gate, never a fresh epoch.
		let mut options = OpenOptions::new();
		options.read(true).write(true).create(true).truncate(false);
		#[cfg(unix)]
		{
			use std::os::unix::fs::OpenOptionsExt;
			options.mode(0o600);
		}
		let file = options
			.open(self.directory.join("ledger.lock"))
			.map_err(storage_error)?;
		file.lock().map_err(storage_error)?;
		Ok(file)
	}
	pub fn load(&self) -> Result<Ledger> {
		let _guard = self.lock()?;
		self.load_locked()
	}
	fn load_locked(&self) -> Result<Ledger> {
		let ledger: Ledger = read(&self.directory.join("ledger.cbor"))?;
		let epoch: uuid::Uuid = read(&self.directory.join("epoch.cbor"))?;
		if ledger.epoch != epoch {
			return Err(Error::Forbidden);
		}
		ledger.validate(&self.home)?;
		Ok(ledger)
	}
	pub fn initialize(&self, units: &[Unit]) -> Result<()> {
		std::fs::create_dir_all(&self.directory).map_err(storage_error)?;
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			std::fs::set_permissions(&self.directory, std::fs::Permissions::from_mode(0o700))
				.map_err(storage_error)?;
		}
		let _guard = self.lock()?;
		if self.directory.join("ledger.cbor").exists() || self.directory.join("epoch.cbor").exists()
		{
			return Err(Error::Conflict(
				"external memory ledger already exists; never replace its epoch".into(),
			));
		}
		let mut ledger = Ledger::new(self.home.clone());
		for unit in units {
			ledger.observe(unit)?;
		}
		// A crash after this permanent anchor leaves a closed gate. Initialization
		// can never replace a prior epoch when its ledger file is missing.
		atomic_write(&self.directory.join("epoch.cbor"), &ledger.epoch)?;
		atomic_write(&self.directory.join("ledger.cbor"), &ledger)
	}
	pub fn gate(&self, restoring: bool, epoch: uuid::Uuid) -> Result<Ledger> {
		let _guard = self.lock()?;
		let mut ledger = self.load_locked()?;
		if ledger.epoch != epoch {
			return Err(Error::Forbidden);
		}
		ledger.restoring = restoring;
		atomic_write(&self.directory.join("ledger.cbor"), &ledger)?;
		Ok(ledger)
	}
	pub fn advance_restore(&self, epoch: uuid::Uuid, unit: &Unit) -> Result<()> {
		let _guard = self.lock()?;
		let mut ledger = self.load_locked()?;
		if !ledger.restoring || ledger.epoch != epoch {
			return Err(Error::Forbidden);
		}
		ledger.observe(unit)?;
		atomic_write(&self.directory.join("ledger.cbor"), &ledger)
	}
}
impl MemoryRecovery for FileRecovery {
	fn require_serving(&self) -> aidash_application::Result<()> {
		let ledger = self
			.load()
			.map_err(|_| aidash_application::Error::SemanticUnavailable)?;
		if ledger.restoring {
			return Err(aidash_application::Error::SemanticUnavailable);
		}
		Ok(())
	}
	fn epoch(&self) -> aidash_application::Result<uuid::Uuid> {
		Ok(self.load()?.epoch)
	}
	fn require_current(&self, unit: &Unit) -> aidash_application::Result<()> {
		let ledger = self
			.load()
			.map_err(|_| aidash_application::Error::SemanticUnavailable)?;
		if ledger.restoring || !ledger.matches(unit)? {
			return Err(aidash_application::Error::SemanticUnavailable);
		}
		Ok(())
	}
	fn observe_many(&self, units: &[Unit]) -> aidash_application::Result<()> {
		let result = (|| {
			let _guard = self.lock()?;
			let mut ledger = self.load_locked()?;
			if ledger.restoring {
				return Err(Error::SemanticUnavailable);
			}
			for unit in units {
				ledger.observe(unit)?;
			}
			atomic_write(&self.directory.join("ledger.cbor"), &ledger)
		})();
		result.map_err(Into::into)
	}
}
fn storage_error(_: std::io::Error) -> Error {
	Error::SemanticUnavailable
}

pub(crate) fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
	let file = File::open(path).map_err(storage_error)?;
	if file.metadata().map_err(storage_error)?.len() > MAX_FILE_BYTES {
		return Err(Error::Invalid(
			"memory recovery file exceeds its declared size cap".into(),
		));
	}
	let mut data = Vec::new();
	file.take(MAX_FILE_BYTES + 1)
		.read_to_end(&mut data)
		.map_err(storage_error)?;
	if data.len() < 40
		|| data.len() as u64 > MAX_FILE_BYTES
		|| &data[..8] != MAGIC
		|| Sha256::digest(&data[40..]).as_slice() != &data[8..40]
	{
		return Err(Error::Invalid(
			"invalid or truncated new-format memory recovery file".into(),
		));
	}
	let mut cursor = std::io::Cursor::new(&data[40..]);
	let value = ciborium::de::from_reader(&mut cursor)
		.map_err(|_| Error::Invalid("invalid typed CBOR memory recovery file".into()))?;
	if cursor.position() as usize != data.len() - 40 {
		return Err(Error::Invalid("trailing memory recovery data".into()));
	}
	Ok(value)
}
pub(crate) fn atomic_write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
	let mut data = Vec::new();
	ciborium::ser::into_writer(value, &mut data)
		.map_err(|_| Error::Invalid("memory recovery serialization failed".into()))?;
	if data.len() as u64 + 40 > MAX_FILE_BYTES {
		return Err(Error::Invalid(
			"memory recovery file exceeds its declared size cap".into(),
		));
	}
	let parent = path.parent().ok_or(Error::Forbidden)?;
	let temporary = parent.join(format!(".memory-{}.pending", uuid::Uuid::new_v4()));
	struct Pending(PathBuf);
	impl Drop for Pending {
		fn drop(&mut self) {
			let _ = std::fs::remove_file(&self.0);
		}
	}
	let pending = Pending(temporary.clone());
	let mut options = OpenOptions::new();
	options.write(true).create_new(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt;
		options.mode(0o600);
	}
	let mut file = options.open(&temporary).map_err(storage_error)?;
	file.write_all(MAGIC).map_err(storage_error)?;
	file.write_all(&Sha256::digest(&data))
		.map_err(storage_error)?;
	file.write_all(&data).map_err(storage_error)?;
	file.sync_all().map_err(storage_error)?;
	std::fs::rename(&temporary, path).map_err(storage_error)?;
	File::open(parent)
		.and_then(|directory| directory.sync_all())
		.map_err(storage_error)?;
	drop(pending);
	Ok(())
}
