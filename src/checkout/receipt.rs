//! Private proof of a successful native jj add. Pending is written before the
//! command; only its observed zero exit and verified filesystem identity can
//! turn that attempt into Ready.
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum State {
    Pending,
    Failed,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Identity {
    root_dev: u64,
    root_ino: u64,
    marker_dev: u64,
    marker_ino: u64,
}

pub(super) fn identity(path: &Path) -> Result<Identity, String> {
    let root = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let marker_path = path.join(".jj");
    let marker = fs::symlink_metadata(&marker_path)
        .map_err(|e| format!("{}: {e}", marker_path.display()))?;
    if !root.is_dir() || !marker.is_dir() {
        return Err(format!("invalid jj checkout at {}", path.display()));
    }
    Ok(Identity {
        root_dev: root.dev(),
        root_ino: root.ino(),
        marker_dev: marker.dev(),
        marker_ino: marker.ino(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    version: u32,
    pub store: String,
    pub name: String,
    pub destination: String,
    pub base: String,
    pub configured_path: String,
    pub attempt: String,
    pub state: State,
    pub identity: Option<Identity>,
}

impl Receipt {
    pub fn pending(store: &str, name: &str, dest: &str, base: &str, configured: &str) -> Self {
        Self {
            version: 1,
            store: store.into(),
            name: name.into(),
            destination: dest.into(),
            base: base.into(),
            configured_path: configured.into(),
            attempt: format!(
                "{}-{}-{}",
                std::process::id(),
                timestamp(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            state: State::Pending,
            identity: None,
        }
    }

    pub fn matches_new(&self, dest: &str, base: &str, configured: &str) -> bool {
        self.destination == dest && self.base == base && self.configured_path == configured
    }

    pub fn ready_identity(&self, path: &Path) -> Result<(), String> {
        if self.state != State::Ready || self.identity.as_ref() != Some(&identity(path)?) {
            return Err(format!(
                "jj checkout at {} no longer matches its Ready receipt; inspect it manually",
                path.display()
            ));
        }
        Ok(())
    }

    pub fn ready(&mut self, path: &Path) -> Result<(), String> {
        self.identity = Some(identity(path)?);
        self.state = State::Ready;
        Ok(())
    }

    pub fn failed(&mut self) {
        self.state = State::Failed;
    }
}

fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
thread_local! {
    static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_root(root: Option<PathBuf>) {
    TEST_ROOT.with(|slot| *slot.borrow_mut() = root);
}

fn directory() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(path) = TEST_ROOT.with(|slot| slot.borrow().clone()) {
        return Ok(path);
    }
    let home = std::env::var_os("HOME").ok_or("HOME is unavailable for jj receipt storage")?;
    let home =
        fs::canonicalize(&home).map_err(|e| format!("cannot resolve HOME for jj receipts: {e}"))?;
    Ok(home.join(".herdr/jj-workspaces/.native-receipts-v1"))
}

fn digest_from(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn digest(bytes: &[u8]) -> u64 {
    digest_from(0xcbf29ce484222325, bytes)
}

fn name_id(store: &str, name: &str) -> String {
    // Length framing prevents concatenation ambiguity; stored fields are always checked.
    let hash = digest_from(0xcbf29ce484222325, &(store.len() as u64).to_le_bytes());
    let hash = digest_from(digest_from(hash, store.as_bytes()), name.as_bytes());
    format!("{hash:016x}")
}

fn prepare_directory(path: &Path) -> Result<(), String> {
    let mut missing = Vec::new();
    let mut cursor = path;
    loop {
        match fs::symlink_metadata(cursor) {
            Ok(meta) if meta.is_dir() => break,
            Ok(_) => {
                return Err(format!(
                    "unsafe jj receipt directory {}; inspect manually",
                    cursor.display()
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(cursor.to_path_buf());
                cursor = cursor
                    .parent()
                    .ok_or("jj receipt directory has no parent")?;
            }
            Err(e) => {
                return Err(format!(
                    "cannot inspect jj receipt directory {}: {e}",
                    cursor.display()
                ))
            }
        }
    }
    for dir in missing.iter().rev() {
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => (),
            Err(e) => {
                return Err(format!(
                    "cannot create jj receipt directory {}: {e}",
                    dir.display()
                ))
            }
        }
    }
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "jj receipt directory {} must be a private directory (mode 0700); inspect manually",
            path.display()
        ));
    }
    // Sync the new directory entries and their ancestors before any native
    // mutation. In production the fourth level is HOME; test roots are
    // explicitly scoped to their fixture parent.
    #[cfg(test)]
    let depth = if TEST_ROOT.with(|slot| slot.borrow().is_some()) {
        2
    } else {
        4
    };
    #[cfg(not(test))]
    let depth = 4;
    for dir in path.ancestors().take(depth.max(missing.len() + 1)) {
        File::open(dir)
            .and_then(|fd| fd.sync_all())
            .map_err(|e| format!("cannot sync jj receipt directory {}: {e}", dir.display()))?;
    }
    Ok(())
}

pub(super) fn load(store: &str, name: &str) -> Result<Option<Receipt>, String> {
    let dir = directory()?;
    let path = dir.join(format!("name-{}.json", name_id(store, name)));
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() && meta.permissions().mode() & 0o077 == 0 => (),
        Ok(_) => {
            return Err(format!(
                "unsafe jj receipt directory {}; inspect manually",
                dir.display()
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "cannot inspect jj receipt directory {}: {e}",
                dir.display()
            ))
        }
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read jj receipt {}: {e}", path.display())),
    };
    let record: Receipt = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "corrupt jj receipt {}: {e}; inspect manually",
            path.display()
        )
    })?;
    if record.version != 1
        || record.store != store
        || record.name != name
        || record.destination.is_empty()
        || record.attempt.is_empty()
        || (record.state == State::Ready) != record.identity.is_some()
    {
        return Err(format!(
            "unsupported or mismatched jj receipt {}; inspect manually",
            path.display()
        ));
    }
    Ok(Some(record))
}

pub(super) fn save(record: &Receipt) -> Result<(), String> {
    let directory = directory()?;
    prepare_directory(&directory)?;
    let target = directory.join(format!(
        "name-{}.json",
        name_id(&record.store, &record.name)
    ));
    let temp = directory.join(format!(
        ".{}-{}-{}.tmp",
        name_id(&record.store, &record.name),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| format!("cannot create jj receipt {}: {e}", temp.display()))?;
        serde_json::to_writer(&mut file, record)
            .map_err(|e| format!("cannot encode jj receipt: {e}"))?;
        file.write_all(b"\n")
            .map_err(|e| format!("cannot write jj receipt {}: {e}", temp.display()))?;
        file.sync_all()
            .map_err(|e| format!("cannot sync jj receipt {}: {e}", temp.display()))?;
        fs::rename(&temp, &target)
            .map_err(|e| format!("cannot install jj receipt {}: {e}", target.display()))?;
        File::open(&directory)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| {
                format!(
                    "cannot sync jj receipt directory {}: {e}",
                    directory.display()
                )
            })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[derive(Debug)]
pub(super) struct Leases {
    _first: File,
    _second: File,
}

fn lease_file(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("cannot open jj lease {}: {e}", path.display()))?;
    file.try_lock().map_err(|e| {
        format!(
            "jj checkout lease {} is busy or unavailable: {e}",
            path.display()
        )
    })?;
    let held = file
        .metadata()
        .map_err(|e| format!("jj lease {}: {e}", path.display()))?;
    let current = fs::metadata(path).map_err(|e| format!("jj lease {}: {e}", path.display()))?;
    if held.dev() != current.dev() || held.ino() != current.ino() {
        return Err(format!(
            "jj lease {} changed while locking; inspect manually",
            path.display()
        ));
    }
    Ok(file)
}

pub(super) fn leases(store: &str, name: &str, destination: &Path) -> Result<Leases, String> {
    let dir = directory()?;
    prepare_directory(&dir)?;
    let named = dir.join(format!("name-{}.lock", name_id(store, name)));
    let destination = dir.join(format!(
        "destination-{:016x}.lock",
        digest(destination.as_os_str().as_encoded_bytes())
    ));
    let (first, second) = if named < destination {
        (named, destination)
    } else {
        (destination, named)
    };
    Ok(Leases {
        _first: lease_file(&first)?,
        _second: lease_file(&second)?,
    })
}
