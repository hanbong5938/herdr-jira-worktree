//! Repository identity and native checkout operations. Git's UI adapter lives in
//! `herdr`; jj commands here never pass through herdr's JSON API parser.
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod receipt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Git,
    #[default]
    Jj,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Git => "Git worktree",
            Self::Jj => "jj workspace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub backend: Backend,
    pub key: String,
    /// A reachable source checkout, not the shared storage directory.
    pub root: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// None for jj workspace records predating recorded workspace roots.
    pub path: Option<String>,
    pub open: bool,
    pub is_source: bool,
}

#[derive(Debug, Clone)]
pub struct Listing {
    pub repository: Repository,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
pub enum Selection {
    Existing(Entry),
    New {
        name: String,
        base: String,
        path: String,
    },
}

#[derive(Debug, Clone)]
pub struct Request {
    pub repository: Repository,
    pub selection: Selection,
    pub label: String,
    pub trust_repository: bool,
    pub recovery_acknowledged: bool,
}

impl Request {
    pub fn name(&self) -> &str {
        match &self.selection {
            Selection::Existing(entry) => &entry.name,
            Selection::New { name, .. } => name,
        }
    }
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn text_path(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("non-UTF-8 checkout path: {}", path.display()))
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git for {}: {e}", root.display()))?;
    if !out.status.success() {
        return Err(format!(
            "git repository at {}: {}",
            root.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut text =
        String::from_utf8(out.stdout).map_err(|e| format!("git returned non-UTF-8 output: {e}"))?;
    if text.ends_with('\n') {
        text.pop();
    }
    Ok(text)
}

fn git_branch(root: &Path) -> Result<Option<String>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
        .map_err(|e| format!("cannot run git for {}: {e}", root.display()))?;
    if out.status.code() == Some(1) {
        return Ok(None); // Only a detached HEAD has the documented exit status 1.
    }
    if !out.status.success() {
        return Err(format!(
            "git symbolic-ref at {} failed: {}",
            root.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let refname = String::from_utf8(out.stdout)
        .map_err(|e| format!("git returned non-UTF-8 symbolic ref: {e}"))?;
    let refname = refname.strip_suffix('\n').unwrap_or(&refname);
    let branch = refname.strip_prefix("refs/heads/").ok_or_else(|| {
        format!(
            "Git HEAD at {} does not refer to a branch: {refname}",
            root.display()
        )
    })?;
    if branch.is_empty() {
        return Err(format!(
            "Git HEAD at {} has an empty branch name",
            root.display()
        ));
    }
    Ok(Some(branch.to_owned()))
}

fn git_repository(root: &Path) -> Result<Repository, String> {
    let top = canonical(Path::new(&git(root, &["rev-parse", "--show-toplevel"])?))?;
    if top != root {
        return Err(format!(
            "Git marker at {} is not a Git checkout root",
            root.display()
        ));
    }
    let common = git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let store = canonical(Path::new(&common))?;
    let name = store
        .parent()
        .filter(|_| store.file_name().is_some_and(|s| s == ".git"))
        .unwrap_or(&top)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| top.display().to_string());
    Ok(Repository {
        backend: Backend::Git,
        key: format!("git:{}", store.display()),
        root: text_path(&top)?,
        name,
    })
}

/// The shared store is `.jj/repo` (directory or a pointer file whose path is
/// relative to `.jj`). Resolve it separately from the selected workspace root.
fn jj_store(root: &Path) -> Result<PathBuf, String> {
    let jj = root.join(".jj");
    if !jj.is_dir() {
        return Err(format!("malformed jj workspace marker: {}", jj.display()));
    }
    let repo = jj.join("repo");
    let target = if repo.is_file() {
        let bytes = fs::read(&repo).map_err(|e| format!("{}: {e}", repo.display()))?;
        let text = std::str::from_utf8(&bytes).map_err(|e| {
            format!(
                "{}: unsupported non-UTF-8 repository pointer: {e}",
                repo.display()
            )
        })?;
        jj.join(text)
    } else {
        repo
    };
    let store = canonical(&target)?;
    if !store.is_dir() {
        return Err(format!(
            "jj repository store is not a directory: {}",
            store.display()
        ));
    }
    Ok(store)
}

fn jj_bin() -> String {
    std::env::var("JJ_BIN_PATH").unwrap_or_else(|_| "jj".into())
}

fn require_jj_version(source: &str) -> Result<(), String> {
    let out = Command::new(jj_bin())
        .arg("--version")
        .output()
        .map_err(|e| format!("cannot run jj for {source}: {e}; install jj 0.38 or newer"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot read jj version: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let version = String::from_utf8_lossy(&out.stdout);
    let version = version.split_whitespace().last().unwrap_or("");
    let mut pieces = version.split('.');
    let major = pieces.next().and_then(|v| v.parse::<u64>().ok());
    let minor = pieces.next().and_then(|v| v.parse::<u64>().ok());
    if !matches!((major, minor), (Some(1..), _) | (Some(0), Some(38..))) {
        return Err(format!(
            "jj {version} is unsupported; install jj 0.38 or newer"
        ));
    }
    Ok(())
}

fn jj(source: &str, args: &[&str], no_snapshot: bool) -> Result<String, String> {
    let mut cmd = Command::new(jj_bin());
    cmd.args(["--no-pager", "--color=never", "-R", source]);
    if no_snapshot {
        cmd.arg("--ignore-working-copy");
    }
    let out = cmd.args(args).output().map_err(|e| {
        format!("cannot run jj for {source}: {e}; install jj 0.38 or newer with workspace JSON templates")
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "jj {} in {source} failed: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("jj returned non-UTF-8 output: {e}"))
}

// 0.38 supports json(name) but not WorkspaceRef.root or map literals. Query
// each recorded root by its serialized name; unavailable historical roots
// remain visible as None rather than guessing a checkout location.
const WORKSPACE_TEMPLATE: &str = "json(name) ++ \"\\n\"";

fn jj_entries(source: &str) -> Result<Vec<Entry>, String> {
    let output = jj(
        source,
        &["workspace", "list", "-T", WORKSPACE_TEMPLATE],
        true,
    )?;
    let source = canonical(Path::new(source))?;
    let mut entries = Vec::new();
    for line in output.lines() {
        let name: String = serde_json::from_str(line)
            .map_err(|e| format!("jj workspace template produced invalid JSON name: {e}"))?;
        if name.is_empty() {
            return Err("jj workspace template returned an empty name".into());
        }
        let path = jj(
            source.to_str().ok_or("non-UTF-8 jj source path")?,
            &["workspace", "root", "--name", &name],
            true,
        )
        .ok()
        .map(|p| p.strip_suffix('\n').unwrap_or(&p).to_string())
        .filter(|p| !p.is_empty());
        let is_source = path
            .as_ref()
            .is_some_and(|p| canonical(Path::new(p)).ok().as_ref() == Some(&source));
        entries.push(Entry {
            name,
            path,
            open: false,
            is_source,
        });
    }
    if entries.is_empty() {
        return Err("jj workspace list returned no workspace records; check jj >= 0.38 and workspace template support".into());
    }
    Ok(entries)
}

fn jj_repository(root: &Path) -> Result<Repository, String> {
    let store = jj_store(root)?;
    let source = text_path(root)?;
    require_jj_version(&source)?;
    jj_entries(&source)?; // Native loading and required template capability.
    let main = store.parent().and_then(Path::parent).unwrap_or(root);
    let name = main
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    Ok(Repository {
        backend: Backend::Jj,
        key: format!("jj:{}", store.display()),
        root: source,
        name,
    })
}

fn marker_present(root: &Path, marker: &str) -> Result<bool, String> {
    let path = root.join(marker);
    match fs::symlink_metadata(&path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!(
            "cannot inspect checkout marker {}: {e}",
            path.display()
        )),
    }
}

/// Nearest filesystem checkout marker. This intentionally does not run Git or jj:
/// even a damaged marker prevents an ancestor from claiming this checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckoutBoundary {
    pub root: PathBuf,
    pub has_jj: bool,
    pub has_git: bool,
}

pub(crate) fn nearest_checkout_boundary(path: &Path) -> Result<Option<CheckoutBoundary>, String> {
    let path = canonical(path)?;
    for root in path.ancestors() {
        let has_jj = marker_present(root, ".jj")?;
        let has_git = marker_present(root, ".git")?;
        if has_jj || has_git {
            return Ok(Some(CheckoutBoundary {
                root: root.to_path_buf(),
                has_jj,
                has_git,
            }));
        }
    }
    Ok(None)
}

/// The closest enclosing checkout boundary wins. A jj Git storage backend is
/// not a Git checkout; only a `.git` marker at the same root makes Git usable.
pub fn resolve_repository(path: &str, prefer: Backend) -> Result<Repository, String> {
    let boundary = nearest_checkout_boundary(Path::new(path))?
        .ok_or_else(|| format!("no Git or jj checkout contains {path}"))?;
    let root = &boundary.root;
    if boundary.has_jj {
        // Broken pointers are not silently interpreted as absent markers.
        jj_store(root)?;
        if boundary.has_git && prefer == Backend::Git {
            return git_repository(root);
        }
        match jj_repository(root) {
            Ok(repo) => return Ok(repo),
            Err(error) if boundary.has_git && error.contains("cannot run jj for") => {
                return git_repository(root)
            }
            Err(error) => return Err(error),
        }
    }
    git_repository(root)
}

pub fn list(repository: &Repository) -> Result<Vec<Entry>, String> {
    match repository.backend {
        Backend::Jj => jj_entries(&repository.root),
        Backend::Git => Err("Git listing is provided by herdr".into()),
    }
}

/// Verify an entered/reopened checkout against the shared store and its native
/// workspace identity, rather than trusting a stale picker path or branch.
pub fn validate_existing(repository: &Repository, name: &str, path: &str) -> Result<Entry, String> {
    let root = canonical(Path::new(path))?;
    let actual = resolve_repository(&text_path(&root)?, repository.backend)?;
    if actual.backend != repository.backend
        || actual.key != repository.key
        || actual.root != text_path(&root)?
    {
        return Err(format!(
            "{} is not a {} in {}",
            root.display(),
            repository.backend.label(),
            repository.name
        ));
    }
    let matches = match repository.backend {
        Backend::Git => {
            // Detached Git is selectable by path; other symbolic-ref errors are fatal.
            let actual_name = git_branch(&root)?.unwrap_or_else(|| {
                root.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            actual_name == name
        }
        Backend::Jj => {
            let entries = jj_entries(&actual.root)?;
            let recorded = entries.iter().find(|entry| entry.name == name);
            if let Some(Some(path)) = recorded.map(|entry| entry.path.as_ref()) {
                if canonical(Path::new(path))? != root {
                    return Err(format!(
                        "jj workspace '{name}' is recorded at {path}, not {}",
                        root.display()
                    ));
                }
            }
            // A workspace list record alone is not a working-copy identity:
            // distinct names can share @. Check every candidate, including roots
            // already recorded by jj, with the supported 0.38 template.
            let output = jj(
                &actual.root,
                &[
                    "workspace",
                    "list",
                    "-T",
                    "if(target.current_working_copy(), json(name) ++ \"\\n\")",
                ],
                true,
            )?;
            let mut candidates = output
                .lines()
                .map(serde_json::from_str::<String>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("jj current workspace template: {e}"))?;
            if candidates.len() > 1 {
                return Err(format!("ambiguous jj workspace identity at {}: multiple names share the current working-copy commit; cannot validate '{name}'", root.display()));
            }
            recorded.is_some() && candidates.pop().as_deref() == Some(name)
        }
    };
    if !matches {
        return Err(format!(
            "{} is not the selected {} '{}'",
            root.display(),
            repository.backend.label(),
            name
        ));
    }
    Ok(Entry {
        name: name.into(),
        path: Some(text_path(&root)?),
        open: false,
        is_source: repository.root == text_path(&root)?,
    })
}

fn path_slug(name: &str) -> Result<String, String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.starts_with('-')
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        return Err(
            "jj workspace name must be a nonempty single path component, without a leading '-'"
                .into(),
        );
    }
    let slug = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    if slug == "." || slug == ".." {
        return Err("invalid jj workspace name".into());
    }
    Ok(slug)
}

fn default_destination(repository: &Repository, name: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .ok_or("HOME is unavailable for the default jj workspace destination")?;
    // Stable FNV-1a digest: not process-randomized, and namespace includes the
    // entire canonical backend-qualified common store key.
    let hash = repository
        .key
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, byte| {
            (h ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    let repo_slug = path_slug(&repository.name).unwrap_or_else(|_| "repository".into());
    Ok(PathBuf::from(home)
        .join(".herdr/jj-workspaces")
        .join(format!("{repo_slug}-{hash:016x}"))
        .join(path_slug(name)?))
}

fn desired_path(repository: &Repository, name: &str, configured: &str) -> Result<PathBuf, String> {
    let path = if configured.is_empty() {
        default_destination(repository, name)?
    } else {
        PathBuf::from(configured)
    };
    if !path.is_absolute() {
        return Err(format!(
            "jj workspace destination must be absolute: {}",
            path.display()
        ));
    }
    let parent = path
        .parent()
        .ok_or("jj workspace destination has no parent")?;
    fs::create_dir_all(parent).map_err(|e| {
        format!(
            "cannot create jj workspace parent {}: {e}",
            parent.display()
        )
    })?;
    Ok(canonical(parent)?.join(
        path.file_name()
            .ok_or("jj workspace destination has no name")?,
    ))
}

fn path_occupied(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!(
            "cannot inspect jj destination {}: {e}",
            path.display()
        )),
    }
}

fn check_jj_source(repository: &Repository) -> Result<(), String> {
    let source = canonical(Path::new(&repository.root))?;
    if source != Path::new(&repository.root)
        || format!("jj:{}", jj_store(&source)?.display()) != repository.key
    {
        return Err(format!(
            "jj source {} changed its native store or root; resolve it again",
            repository.root
        ));
    }
    require_jj_version(&repository.root)
}

fn recorded_checkout_path(entries: &[Entry], name: &str, destination: &Path) -> Result<(), String> {
    let recorded = entries
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            format!(
                "jj workspace '{name}' is missing from the native listing; checkout retained at {}",
                destination.display()
            )
        })?;
    let path = recorded.path.as_deref().ok_or_else(|| {
        format!(
            "jj workspace '{name}' has no recorded path; checkout retained at {}",
            destination.display()
        )
    })?;
    if canonical(Path::new(path))? != destination {
        return Err(format!(
            "jj workspace '{name}' is recorded at {path}, not {}; checkout retained",
            destination.display()
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub struct PreparedJjCheckout {
    pub entry: Entry,
    _leases: receipt::Leases,
}

fn manual_recovery(name: &str, path: &str) -> String {
    format!("jj workspace '{name}' at {path} has an unresolved native add attempt; inspect that path and run `jj status` from inside the checkout, then select Existing and explicitly approve recovery; no native add will be retried")
}

/// Read-only indication that native jj may have left an incomplete workspace.
/// Call only after validating the selected path/native identity.
pub fn recovery_warning(repository: &Repository, entry: &Entry) -> Result<Option<String>, String> {
    if repository.backend == Backend::Git {
        return Ok(None);
    }
    let path = entry
        .path
        .as_deref()
        .ok_or("jj workspace has no path for recovery inspection")?;
    let selected = validate_existing(repository, &entry.name, path)?;
    let path = selected.path.as_deref().unwrap();
    match receipt::load(&repository.key, &entry.name)? {
        None => Ok(None),
        Some(record) if record.destination != path => Err(format!(
            "jj receipt for '{}' points to {}, not {path}; inspect manually",
            entry.name, record.destination
        )),
        Some(record) if record.state == receipt::State::Ready => {
            record.ready_identity(Path::new(path))?;
            Ok(None)
        }
        Some(_) => Ok(Some(manual_recovery(&entry.name, path))),
    }
}

/// Hold native name and destination leases until herdr finishes attaching UI.
/// A failed UI attach cannot undo native creation or its durable Ready proof.
pub fn ensure_jj(request: &Request) -> Result<PreparedJjCheckout, String> {
    let repository = &request.repository;
    if repository.backend != Backend::Jj {
        return Err("expected a jj repository".into());
    }
    check_jj_source(repository)?;
    match &request.selection {
        Selection::Existing(selected) => {
            let path = selected.path.as_deref().ok_or_else(|| {
                format!(
                    "jj workspace '{}' has no recorded path; enter its existing checkout path",
                    selected.name
                )
            })?;
            let dest = canonical(Path::new(path))?;
            let dest_str = text_path(&dest)?;
            let leases = receipt::leases(&repository.key, &selected.name, &dest)?;
            let entry = validate_existing(repository, &selected.name, &dest_str)?;
            if let Some(record) = receipt::load(&repository.key, &selected.name)? {
                if record.destination != dest_str {
                    return Err(format!(
                        "jj receipt for '{}' points to {}, not {dest_str}; inspect manually",
                        selected.name, record.destination
                    ));
                }
                if record.state == receipt::State::Ready {
                    record.ready_identity(&dest)?;
                } else if !request.recovery_acknowledged {
                    return Err(manual_recovery(&selected.name, &dest_str));
                }
            }
            Ok(PreparedJjCheckout {
                entry,
                _leases: leases,
            })
        }
        Selection::New { name, base, path } => {
            path_slug(name)?;
            let dest = desired_path(repository, name, path)?;
            let dest_str = text_path(&dest)?;
            let leases = receipt::leases(&repository.key, name, &dest)?;
            let proof = receipt::load(&repository.key, name)?;
            if let Some(record) = &proof {
                if !record.matches_new(&dest_str, base, path) {
                    return Err(format!("jj workspace '{name}' already has a different native attempt at {}; inspect manually and select Existing", record.destination));
                }
            }
            let entries = jj_entries(&repository.root)?;
            let recorded = entries.iter().find(|entry| entry.name == *name);
            if let Some(record) = proof {
                if record.state != receipt::State::Ready {
                    return Err(manual_recovery(name, &dest_str));
                }
                recorded_checkout_path(&entries, name, &dest)?;
                let entry = validate_existing(repository, name, &dest_str)?;
                record.ready_identity(&dest)?;
                return Ok(PreparedJjCheckout {
                    entry,
                    _leases: leases,
                });
            }
            if let Some(existing) = recorded {
                return Err(format!("jj workspace '{name}' already exists at {}; select Existing after inspecting it", existing.path.as_deref().unwrap_or("(unknown path)")));
            }
            if path_occupied(&dest)? {
                return Err(format!("jj workspace destination already exists: {dest_str}; inspect it and select Existing rather than creating over it"));
            }
            let mut record =
                receipt::Receipt::pending(&repository.key, name, &dest_str, base, path);
            receipt::save(&record)?; // No native mutation until Pending is durable.
            let mut args = vec!["workspace", "add", "--name", name.as_str()];
            if !base.is_empty() {
                args.extend(["-r", base.as_str()]);
            }
            args.push(&dest_str);
            if let Err(error) = jj(&repository.root, &args, false) {
                record.failed();
                let retention = match receipt::save(&record) {
                    Ok(()) => String::new(),
                    Err(e) => {
                        format!("; Failed receipt could not be persisted ({e}); Pending remains")
                    }
                };
                return Err(format!(
                    "{error}; checkout at {dest_str} was retained for manual inspection{retention}"
                ));
            }
            recorded_checkout_path(
                &jj_entries(&repository.root)
                    .map_err(|e| format!("{e}; native checkout retained at {dest_str}, Pending receipt requires manual inspection"))?,
                name,
                &dest,
            ).map_err(|e| format!("{e}; Pending receipt requires manual inspection"))?;
            let entry = validate_existing(repository, name, &dest_str)
                .map_err(|e| format!("{e}; native checkout retained at {dest_str}, Pending receipt requires manual inspection"))?;
            record.ready(&dest).map_err(|e| format!("{e}; native checkout retained at {dest_str}, Pending receipt requires manual inspection"))?;
            receipt::save(&record).map_err(|e| format!("{e}; native checkout retained at {dest_str}; inspect its receipt before reopening"))?;
            Ok(PreparedJjCheckout {
                entry,
                _leases: leases,
            })
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, MutexGuard};

    static ENV: Mutex<()> = Mutex::new(());
    static NEXT: AtomicU64 = AtomicU64::new(0);

    pub(crate) struct Fixture {
        _lock: MutexGuard<'static, ()>,
        pub(crate) dir: PathBuf,
        pub(crate) root: PathBuf,
        old_config: Option<OsString>,
    }

    impl Fixture {
        pub(crate) fn new(colocated: bool) -> Option<Self> {
            if Command::new(jj_bin()).arg("--version").output().is_err() {
                return None;
            }
            let lock = ENV.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!(
                "jira-jj-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            let dir = fs::canonicalize(dir).unwrap();
            let config = dir.join("config.toml");
            fs::write(
                &config,
                "user.name = 'Test'\nuser.email = 'test@example.invalid'\n",
            )
            .unwrap();
            let old_config = std::env::var_os("JJ_CONFIG");
            std::env::set_var("JJ_CONFIG", &config);
            let root = dir.join("source");
            let option = if colocated {
                "--colocate"
            } else {
                "--no-colocate"
            };
            let output = Command::new(jj_bin())
                .args(["--no-pager", "git", "init", option])
                .arg(&root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            receipt::set_test_root(Some(dir.join("receipt-state")));
            Some(Self {
                _lock: lock,
                dir,
                root,
                old_config,
            })
        }

        fn command(&self, source: &Path, args: &[&str]) -> String {
            let output = Command::new(jj_bin())
                .args(["--no-pager", "--color=never", "-R"])
                .arg(source)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }

        pub(crate) fn request(&self, name: &str, path: &Path, base: &str) -> Request {
            Request {
                repository: resolve_repository(self.root.to_str().unwrap(), Backend::Jj).unwrap(),
                selection: Selection::New {
                    name: name.into(),
                    path: path.to_str().unwrap().into(),
                    base: base.into(),
                },
                label: name.into(),
                trust_repository: false,
                recovery_acknowledged: false,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            receipt::set_test_root(None);
            match &self.old_config {
                Some(value) => std::env::set_var("JJ_CONFIG", value),
                None => std::env::remove_var("JJ_CONFIG"),
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn native_list_add_reopen_and_parent_revision_are_independent_of_bookmarks() {
        let Some(f) = Fixture::new(false) else { return };
        let root = f.root.as_path();
        f.command(root, &["new"]);
        let source_commit = f.command(
            root,
            &[
                "--ignore-working-copy",
                "log",
                "-G",
                "-r",
                "@",
                "-T",
                "commit_id",
            ],
        );
        let source_parent = f.command(
            root,
            &[
                "--ignore-working-copy",
                "log",
                "-G",
                "-r",
                "@-",
                "-T",
                "commit_id",
            ],
        );
        let path = f.dir.join("space ünicode");
        let request = f.request("feature name", &path, "");
        let created = ensure_jj(&request).unwrap().entry;
        assert_eq!(created.path.as_deref(), path.to_str());
        assert!(list(&request.repository)
            .unwrap()
            .iter()
            .any(|e| e.name == "feature name"));
        let parent = f.command(
            &path,
            &[
                "--ignore-working-copy",
                "log",
                "-G",
                "-r",
                "@-",
                "-T",
                "commit_id",
            ],
        );
        assert_eq!(parent, source_parent);
        assert_ne!(parent, source_commit);
        fs::write(path.join("kept.txt"), "uncommitted content").unwrap();
        assert_eq!(ensure_jj(&request).unwrap().entry.path, created.path);
        assert_eq!(
            fs::read_to_string(path.join("kept.txt")).unwrap(),
            "uncommitted content"
        );
        let on_source = f.dir.join("explicit-base");
        let explicit = f.request("explicit", &on_source, "@");
        ensure_jj(&explicit).unwrap();
        let explicit_parent = f.command(
            &on_source,
            &[
                "--ignore-working-copy",
                "log",
                "-G",
                "-r",
                "@-",
                "-T",
                "commit_id",
            ],
        );
        assert_eq!(explicit_parent, source_commit);
        let escaped = f.dir.join("path\nwith spaces");
        ensure_jj(&f.request("quoted \"name\"", &escaped, "")).unwrap();
        let listed = list(&request.repository).unwrap();
        assert_eq!(
            listed
                .iter()
                .find(|e| e.name == "quoted \"name\"")
                .unwrap()
                .path
                .as_deref(),
            escaped.to_str()
        );
    }

    #[test]
    fn linked_store_identity_colocation_and_nearest_nested_boundary() {
        let Some(f) = Fixture::new(true) else { return };
        let base = resolve_repository(f.root.to_str().unwrap(), Backend::Jj).unwrap();
        assert_eq!(base.backend, Backend::Jj);
        assert_eq!(
            resolve_repository(f.root.to_str().unwrap(), Backend::Git)
                .unwrap()
                .backend,
            Backend::Git
        );
        let linked = f.dir.join("linked");
        let entry = ensure_jj(&f.request("linked", &linked, "")).unwrap().entry;
        let linked_repo = resolve_repository(linked.to_str().unwrap(), Backend::Jj).unwrap();
        assert_eq!(linked_repo.key, base.key);
        assert_ne!(linked_repo.root, base.root);
        assert!(linked.join(".jj/repo").is_file());
        assert_eq!(
            validate_existing(&base, &entry.name, linked.to_str().unwrap())
                .unwrap()
                .name,
            "linked"
        );
        let alias = f.dir.join("alias");
        std::os::unix::fs::symlink(&linked, &alias).unwrap();
        assert_eq!(
            resolve_repository(alias.to_str().unwrap(), Backend::Jj)
                .unwrap()
                .key,
            base.key
        );
        let nested = linked.join("independent-git");
        let output = Command::new("git")
            .arg("init")
            .arg(&nested)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            resolve_repository(nested.to_str().unwrap(), Backend::Jj)
                .unwrap()
                .backend,
            Backend::Git
        );
        fs::write(linked.join(".jj/repo"), b"missing-store").unwrap();
        assert!(resolve_repository(linked.to_str().unwrap(), Backend::Git).is_err());
    }

    #[test]
    fn native_rejects_wrong_workspace_path_and_destination_conflicts() {
        let Some(f) = Fixture::new(false) else { return };
        let target = f.dir.join("first");
        let request = f.request("first", &target, "");
        ensure_jj(&request).unwrap();
        let other = f.dir.join("second");
        ensure_jj(&f.request("second", &other, "")).unwrap();
        assert!(validate_existing(&request.repository, "first", other.to_str().unwrap()).is_err());
        let conflict = f.request("first", &other, "");
        assert!(ensure_jj(&conflict)
            .unwrap_err()
            .contains("different native attempt"));
        assert!(ensure_jj(&f.request("third", &target, ""))
            .unwrap_err()
            .contains("already exists"));
        assert_eq!(list(&request.repository).unwrap().len(), 3);
    }

    #[test]
    fn deleted_native_workspace_root_stays_listed_without_recreation() {
        let Some(f) = Fixture::new(false) else { return };
        let checkout = f.dir.join("removed");
        let request = f.request("removed", &checkout, "");
        ensure_jj(&request).unwrap();
        fs::remove_dir_all(&checkout).unwrap();
        let entries = list(&request.repository).unwrap();
        assert!(entries.iter().any(|entry| entry.name == "removed"));
        assert!(
            validate_existing(&request.repository, "removed", checkout.to_str().unwrap()).is_err()
        );
        assert!(ensure_jj(&request).is_err());
        assert!(!checkout.exists());
    }

    #[test]
    fn deterministic_destination_is_namespaced_by_common_store() {
        let Some(f) = Fixture::new(false) else { return };
        let first = resolve_repository(f.root.to_str().unwrap(), Backend::Jj).unwrap();
        assert_eq!(
            default_destination(&first, "issue-1").unwrap(),
            default_destination(&first, "issue-1").unwrap()
        );
        let second = Repository {
            key: format!("{}-other", first.key),
            ..first.clone()
        };
        assert_ne!(
            default_destination(&first, "issue-1").unwrap(),
            default_destination(&second, "issue-1").unwrap()
        );
        assert!(default_destination(&first, "../bad").is_err());
    }

    #[test]
    fn native_custom_destination_cannot_be_claimed_by_another_store_or_dangling_link() {
        let Some(f) = Fixture::new(false) else { return };
        let target = f.dir.join("shared-destination");
        let first = f.request("first", &target, "");
        ensure_jj(&first).unwrap();
        let second_root = f.dir.join("second-source");
        let out = Command::new(jj_bin())
            .args(["--no-pager", "git", "init", "--no-colocate"])
            .arg(&second_root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let second = Request {
            repository: resolve_repository(second_root.to_str().unwrap(), Backend::Jj).unwrap(),
            selection: Selection::New {
                name: "cross-store".into(),
                base: String::new(),
                path: target.to_str().unwrap().into(),
            },
            label: "cross-store".into(),
            trust_repository: false,
            recovery_acknowledged: false,
        };
        assert!(ensure_jj(&second)
            .unwrap_err()
            .contains("destination already exists"));
        assert!(receipt::load(&second.repository.key, "cross-store")
            .unwrap()
            .is_none());
        let dangling = f.dir.join("dangling");
        std::os::unix::fs::symlink(f.dir.join("not-found"), &dangling).unwrap();
        assert!(ensure_jj(&f.request("dangling", &dangling, ""))
            .unwrap_err()
            .contains("destination already exists"));
    }

    #[test]
    fn native_failed_add_never_becomes_ready_even_after_retry() {
        let Some(f) = Fixture::new(false) else { return };
        let path = f.dir.join("partially-created");
        let request = f.request("broken", &path, "no-such-revision");
        let error = ensure_jj(&request).unwrap_err();
        assert!(error.contains("checkout at"));
        let receipt = receipt::load(&request.repository.key, "broken")
            .unwrap()
            .unwrap();
        assert_eq!(receipt.state, receipt::State::Failed);
        assert!(ensure_jj(&request)
            .unwrap_err()
            .contains("unresolved native add"));
        assert_ne!(receipt.state, receipt::State::Ready);
    }

    #[test]
    fn native_pending_requires_explicit_existing_consent_but_never_promotes_itself() {
        let Some(f) = Fixture::new(false) else { return };
        let path = f.dir.join("native-pending");
        let new = f.request("pending", &path, "");
        let entry = ensure_jj(&new).unwrap().entry;
        fs::write(path.join("user-file"), "keep").unwrap();
        let mut receipt = receipt::load(&new.repository.key, "pending")
            .unwrap()
            .unwrap();
        receipt.state = receipt::State::Pending; // Simulate a crash before Ready.
        receipt.identity = None;
        receipt::save(&receipt).unwrap();
        assert!(ensure_jj(&new)
            .unwrap_err()
            .contains("unresolved native add"));
        assert!(recovery_warning(&new.repository, &entry)
            .unwrap()
            .unwrap()
            .contains(path.to_str().unwrap()));
        let mut existing = Request {
            selection: Selection::Existing(entry),
            ..new
        };
        assert!(ensure_jj(&existing)
            .unwrap_err()
            .contains("explicitly approve"));
        existing.recovery_acknowledged = true;
        let prepared = ensure_jj(&existing).unwrap();
        assert_eq!(fs::read_to_string(path.join("user-file")).unwrap(), "keep");
        drop(prepared);
        assert_eq!(
            receipt::load(&existing.repository.key, "pending")
                .unwrap()
                .unwrap()
                .state,
            receipt::State::Pending
        );
    }

    #[test]
    fn native_ready_requires_matching_attempt_and_checkout_inodes() {
        let Some(f) = Fixture::new(false) else { return };
        let path = f.dir.join("same-name");
        let request = f.request("same-name", &path, "");
        let prepared = ensure_jj(&request).unwrap();
        assert!(ensure_jj(&request).unwrap_err().contains("lease"));
        drop(prepared);
        assert!(ensure_jj(&f.request("same-name", &path, "@"))
            .unwrap_err()
            .contains("different native attempt"));
        let marker = path.join(".jj");
        let old = path.join(".jj-old");
        fs::rename(&marker, &old).unwrap();
        fs::create_dir(&marker).unwrap();
        for item in fs::read_dir(&old).unwrap() {
            let item = item.unwrap();
            fs::rename(item.path(), marker.join(item.file_name())).unwrap();
        }
        fs::remove_dir(&old).unwrap();
        assert!(
            validate_existing(&request.repository, "same-name", path.to_str().unwrap()).is_ok()
        );
        assert!(ensure_jj(&request).unwrap_err().contains("Ready receipt"));
    }

    #[test]
    fn native_rejects_swapped_roots_and_corrupt_receipts() {
        let Some(f) = Fixture::new(false) else { return };
        let first = f.dir.join("first");
        let second = f.dir.join("second");
        let request = f.request("first", &first, "");
        ensure_jj(&request).unwrap();
        ensure_jj(&f.request("second", &second, "")).unwrap();
        let intermediate = f.dir.join("intermediate");
        fs::rename(&first, &intermediate).unwrap();
        fs::rename(&second, &first).unwrap();
        fs::rename(&intermediate, &second).unwrap();
        assert!(validate_existing(&request.repository, "first", first.to_str().unwrap()).is_err());
        assert!(ensure_jj(&request).is_err());
        fs::rename(&first, &intermediate).unwrap();
        fs::rename(&second, &first).unwrap();
        fs::rename(&intermediate, &second).unwrap();
        let receipts = f.dir.join("receipt-state");
        let record = fs::read_dir(receipts)
            .unwrap()
            .map(|item| item.unwrap().path())
            .find(|path| {
                path.extension().is_some_and(|ext| ext == "json")
                    && serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap())
                        .unwrap()["name"]
                        == "first"
            })
            .unwrap();
        let mut unsupported: serde_json::Value =
            serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
        unsupported["version"] = serde_json::json!(2);
        fs::write(&record, serde_json::to_vec(&unsupported).unwrap()).unwrap();
        assert!(ensure_jj(&request)
            .unwrap_err()
            .contains("unsupported or mismatched jj receipt"));
        fs::write(&record, b"{ broken receipt").unwrap();
        assert!(ensure_jj(&request)
            .unwrap_err()
            .contains("corrupt jj receipt"));
    }

    #[test]
    fn native_foreign_existing_and_unwritable_state_do_not_create_without_proof() {
        let Some(f) = Fixture::new(false) else { return };
        let foreign = f.dir.join("foreign");
        f.command(
            &f.root,
            &[
                "workspace",
                "add",
                "--name",
                "foreign",
                foreign.to_str().unwrap(),
            ],
        );
        let repository = resolve_repository(f.root.to_str().unwrap(), Backend::Jj).unwrap();
        let entry = validate_existing(&repository, "foreign", foreign.to_str().unwrap()).unwrap();
        assert_eq!(recovery_warning(&repository, &entry).unwrap(), None);
        let existing = Request {
            repository: repository.clone(),
            selection: Selection::Existing(entry),
            label: "foreign".into(),
            trust_repository: false,
            recovery_acknowledged: false,
        };
        assert_eq!(ensure_jj(&existing).unwrap().entry.name, "foreign");
        assert!(ensure_jj(&f.request("foreign", &foreign, ""))
            .unwrap_err()
            .contains("select Existing"));
        let blocked = f.dir.join("not-a-directory");
        fs::write(&blocked, b"no receipts").unwrap();
        receipt::set_test_root(Some(blocked));
        let path = f.dir.join("not-created");
        assert!(ensure_jj(&f.request("never-created", &path, "")).is_err());
        assert!(!path.exists());
        receipt::set_test_root(Some(f.dir.join("receipt-state")));
    }

    #[test]
    fn git_symbolic_ref_disambiguates_branch_and_tag_and_preserves_path_bytes() {
        // Git-only fixture: no jj command, HOME change or per-user metadata.
        let dir = std::env::temp_dir().join(format!(
            "jira-git-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let dir = canonical(&dir).unwrap();
        for suffix in ["trailing space ", "trailing newline\n"] {
            let root = dir.join(suffix);
            let out = Command::new("git")
                .args(["init", "-q", "-b", "release"])
                .arg(&root)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let out = Command::new("git")
                .args(["-C"])
                .arg(&root)
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "initial",
                ])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                git(&root, &["rev-parse", "--show-toplevel"]).unwrap(),
                root.to_str().unwrap()
            );
            let repo = resolve_repository(root.to_str().unwrap(), Backend::Jj).unwrap();
            assert_eq!(repo.backend, Backend::Git);
            let nested = root.join("subdirectory");
            fs::create_dir(&nested).unwrap();
            assert_eq!(
                nearest_checkout_boundary(&nested).unwrap().unwrap().root,
                root
            );
            fs::write(nested.join(".jj"), b"broken marker").unwrap();
            let boundary = nearest_checkout_boundary(&nested).unwrap().unwrap();
            assert_eq!(boundary.root, nested);
            assert!(boundary.has_jj);
            assert!(!boundary.has_git);
            assert!(resolve_repository(nested.to_str().unwrap(), Backend::Git).is_err());
            fs::remove_file(nested.join(".jj")).unwrap();
            let out = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["tag", "release"])
                .output()
                .unwrap();
            assert!(out.status.success());
            assert_eq!(
                validate_existing(&repo, "release", root.to_str().unwrap())
                    .unwrap()
                    .name,
                "release"
            );
            let out = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["switch", "-q", "-c", "nested/branch"])
                .output()
                .unwrap();
            assert!(out.status.success());
            assert!(validate_existing(&repo, "nested/branch", root.to_str().unwrap()).is_ok());
            let out = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["checkout", "-q", "--detach"])
                .output()
                .unwrap();
            assert!(out.status.success());
            assert!(validate_existing(&repo, suffix, root.to_str().unwrap()).is_ok());
        }
        assert!(git_branch(&dir).is_err()); // Not a detached checkout.
        fs::remove_dir_all(dir).unwrap();
    }
}
