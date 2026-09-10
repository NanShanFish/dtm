use crate::package::{
    ApplyMode, EntryKind, PackageError, PackagePlan, SkippedEntry, deployment_name,
    nearest_path_target, reverse_backup_path, target_is_current_entry, validate_package_name,
};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const IGNORED_DIRECTORY_NAMES: &[&str] = &[
    ".git",
    ".next",
    ".venv",
    "__pycache__",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "target",
    "venv",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackEntry {
    pub source: PathBuf,
    pub package_path: PathBuf,
    pub target: PathBuf,
    pub conflicts: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PackPlan {
    pub name: String,
    pub package_root: PathBuf,
    pub entries: Vec<PackEntry>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PackReport {
    pub packed: Vec<PackEntry>,
    pub skipped: Vec<SkippedEntry>,
}

impl PackPlan {
    pub fn load(
        pkgs_dir: &Path,
        package_name: &str,
        inputs: &[PathBuf],
        paths: &BTreeMap<String, String>,
    ) -> Result<Self, PackError> {
        validate_package_name(package_name).map_err(PackError::Package)?;
        if inputs.is_empty() {
            return Err(PackError::NoInput);
        }

        let current_dir = std::env::current_dir().map_err(PackError::CurrentDirectory)?;
        let pkgs_dir = absolute_path(pkgs_dir, &current_dir);
        let package_root = pkgs_dir.join(package_name);
        validate_existing_package(&pkgs_dir, package_name, paths)?;

        let mut sources = BTreeSet::new();
        for input in inputs {
            let input = absolute_path(input, &current_dir);
            collect_input(&input, true, &pkgs_dir, &mut sources)?;
        }
        if sources.is_empty() {
            return Err(PackError::NoFiles);
        }

        let mut entries = Vec::with_capacity(sources.len());
        let mut package_paths = BTreeMap::<PathBuf, PathBuf>::new();
        for source in sources {
            let (path_name, relative) =
                nearest_path_target(&source, paths).ok_or_else(|| PackError::NoPathParent {
                    path: source.clone(),
                })?;
            validate_path_name(path_name)?;
            let mapped = reverse_backup_path(&relative).map_err(PackError::Package)?;
            ensure_pack_path_round_trip(&relative, &mapped)?;
            let package_path = package_root.join(path_name).join(mapped);

            if let Some(first) = package_paths.insert(package_path.clone(), source.clone()) {
                return Err(PackError::TargetCollision {
                    first,
                    second: source,
                    package_path,
                });
            }
            validate_package_destination(&package_root, &package_path)?;
            let conflicts = regular_file_exists(&package_path)?;
            entries.push(PackEntry {
                target: source.clone(),
                source,
                package_path,
                conflicts,
            });
        }
        entries.sort_by(|left, right| left.package_path.cmp(&right.package_path));

        Ok(Self {
            name: package_name.to_owned(),
            package_root,
            entries,
        })
    }

    pub fn retain(mut self, mut keep: impl FnMut(&PackEntry) -> bool) -> Self {
        self.entries.retain(|entry| keep(entry));
        self
    }

    pub fn execute(
        &self,
        paths: &BTreeMap<String, String>,
        template_values: &BTreeMap<String, String>,
    ) -> Result<PackReport, PackError> {
        let pkgs_dir = self
            .package_root
            .parent()
            .ok_or_else(|| PackError::InvalidPackageRoot(self.package_root.clone()))?;
        fs::create_dir_all(pkgs_dir).map_err(|source| PackError::CreateDirectory {
            path: pkgs_dir.to_path_buf(),
            source,
        })?;
        let transaction_root = create_transaction_directory(pkgs_dir, &self.name)?;
        let staged_root = transaction_root.join("new");
        let old_root = transaction_root.join("old");

        if let Err(error) = self.copy_to_stage(&staged_root) {
            let _ = fs::remove_dir_all(&transaction_root);
            return Err(error);
        }

        let mut package_changes = Vec::new();
        if let Err(error) = self.commit_package_files(&staged_root, &old_root, &mut package_changes)
        {
            let rollback = rollback_package_files(&package_changes);
            let _ = fs::remove_dir_all(&transaction_root);
            return Err(with_rollback(error, rollback));
        }

        let package_plan = match PackagePlan::load(pkgs_dir, &self.name, paths) {
            Ok(plan) => plan,
            Err(error) => {
                let rollback = rollback_package_files(&package_changes);
                let _ = fs::remove_dir_all(&transaction_root);
                return Err(with_rollback(PackError::Package(error), rollback));
            }
        };
        if let Err(error) = self.verify_package_plan(&package_plan) {
            let rollback = rollback_package_files(&package_changes);
            let _ = fs::remove_dir_all(&transaction_root);
            return Err(with_rollback(error, rollback));
        }

        let mut removed_sources = Vec::new();
        if let Err(error) = verify_sources_unchanged(&self.entries) {
            let rollback = rollback_package_files(&package_changes);
            let _ = fs::remove_dir_all(&transaction_root);
            return Err(with_rollback(error, rollback));
        }
        for entry in &self.entries {
            if let Err(source) = fs::remove_file(&entry.source) {
                let error = PackError::RemoveSource {
                    path: entry.source.clone(),
                    source,
                };
                let mut rollback = restore_sources(&removed_sources);
                rollback.extend(rollback_package_files(&package_changes));
                let _ = fs::remove_dir_all(&transaction_root);
                return Err(with_rollback(error, rollback));
            }
            removed_sources.push(entry.clone());
        }

        let before = match package_management_state(&package_plan, template_values) {
            Ok(state) => state,
            Err(error) => {
                let mut rollback = restore_sources(&removed_sources);
                rollback.extend(rollback_package_files(&package_changes));
                let _ = fs::remove_dir_all(&transaction_root);
                return Err(with_rollback(PackError::Package(error), rollback));
            }
        };

        let apply_result = package_plan.apply(template_values, ApplyMode::Normal);
        let report = match apply_result {
            Ok(report) => report,
            Err(error) => {
                let mut rollback = remove_newly_installed(&package_plan, template_values, &before);
                rollback.extend(restore_sources(&removed_sources));
                rollback.extend(rollback_package_files(&package_changes));
                let _ = fs::remove_dir_all(&transaction_root);
                return Err(with_rollback(PackError::Package(error), rollback));
            }
        };
        let selected_targets = self
            .entries
            .iter()
            .map(|entry| entry.target.clone())
            .collect::<BTreeSet<_>>();
        let failed = report
            .skipped
            .iter()
            .filter(|entry| {
                selected_targets.contains(&entry.entry.target)
                    && entry.reason != "already up to date"
            })
            .map(|entry| entry.entry.target.clone())
            .collect::<Vec<_>>();
        if !failed.is_empty() {
            let error = PackError::InstallSkipped { paths: failed };
            let mut rollback = remove_newly_installed(&package_plan, template_values, &before);
            rollback.extend(restore_sources(&removed_sources));
            rollback.extend(rollback_package_files(&package_changes));
            let _ = fs::remove_dir_all(&transaction_root);
            return Err(with_rollback(error, rollback));
        }

        fs::remove_dir_all(&transaction_root).map_err(|source| PackError::CleanupTransaction {
            path: transaction_root,
            source,
        })?;
        Ok(PackReport {
            packed: self.entries.clone(),
            skipped: report.skipped,
        })
    }

    fn copy_to_stage(&self, staged_root: &Path) -> Result<(), PackError> {
        for entry in &self.entries {
            let relative = entry
                .package_path
                .strip_prefix(&self.package_root)
                .map_err(|_| PackError::InvalidPackageRoot(entry.package_path.clone()))?;
            let staged = staged_root.join(relative);
            if let Some(parent) = staged.parent() {
                fs::create_dir_all(parent).map_err(|source| PackError::CreateDirectory {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::copy(&entry.source, &staged).map_err(|source| PackError::CopyFile {
                source_path: entry.source.clone(),
                destination: staged,
                source,
            })?;
        }
        Ok(())
    }

    fn commit_package_files(
        &self,
        staged_root: &Path,
        old_root: &Path,
        changes: &mut Vec<PackageChange>,
    ) -> Result<(), PackError> {
        for entry in &self.entries {
            let relative = entry
                .package_path
                .strip_prefix(&self.package_root)
                .map_err(|_| PackError::InvalidPackageRoot(entry.package_path.clone()))?;
            let staged = staged_root.join(relative);
            if let Some(parent) = entry.package_path.parent() {
                fs::create_dir_all(parent).map_err(|source| PackError::CreateDirectory {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            let old = if entry.conflicts {
                let old = old_root.join(relative);
                if let Some(parent) = old.parent() {
                    fs::create_dir_all(parent).map_err(|source| PackError::CreateDirectory {
                        path: parent.to_path_buf(),
                        source,
                    })?;
                }
                fs::rename(&entry.package_path, &old).map_err(|source| PackError::MoveFile {
                    source_path: entry.package_path.clone(),
                    destination: old.clone(),
                    source,
                })?;
                Some(old)
            } else {
                None
            };
            if old.is_some() {
                changes.push(PackageChange {
                    package_path: entry.package_path.clone(),
                    old: old.clone(),
                });
            }
            fs::hard_link(&staged, &entry.package_path).map_err(|source| PackError::MoveFile {
                source_path: staged.clone(),
                destination: entry.package_path.clone(),
                source,
            })?;
            if old.is_none() {
                changes.push(PackageChange {
                    package_path: entry.package_path.clone(),
                    old: None,
                });
            }
            fs::remove_file(&staged).map_err(|source| PackError::MoveFile {
                source_path: staged,
                destination: entry.package_path.clone(),
                source,
            })?;
        }
        Ok(())
    }

    fn verify_package_plan(&self, plan: &PackagePlan) -> Result<(), PackError> {
        for packed in &self.entries {
            let valid = plan.entries.iter().any(|entry| {
                entry.source == packed.package_path
                    && entry.target == packed.target
                    && entry.kind == EntryKind::Symlink
            });
            if !valid {
                return Err(PackError::MappingMismatch {
                    source: packed.source.clone(),
                    package_path: packed.package_path.clone(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PackageChange {
    package_path: PathBuf,
    old: Option<PathBuf>,
}

fn collect_input(
    path: &Path,
    explicit: bool,
    pkgs_dir: &Path,
    files: &mut BTreeSet<PathBuf>,
) -> Result<(), PackError> {
    if path_is_within(path, pkgs_dir) {
        if explicit {
            return Err(PackError::InputContainsPackage {
                input: path.to_path_buf(),
                package: pkgs_dir.to_path_buf(),
            });
        }
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path).map_err(|source| PackError::ReadMetadata {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(PackError::SymbolicLink {
            path: path.to_path_buf(),
        });
    }
    if metadata.is_file() {
        files.insert(path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(PackError::UnsupportedFileType {
            path: path.to_path_buf(),
        });
    }
    if !explicit && is_ignored_directory(path) {
        return Ok(());
    }

    let mut entries = fs::read_dir(path)
        .map_err(|source| PackError::ReadDirectory {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| PackError::ReadDirectory {
            path: path.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        collect_input(&entry.path(), false, pkgs_dir, files)?;
    }
    Ok(())
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    path.starts_with(root)
}

pub fn is_ignored_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| IGNORED_DIRECTORY_NAMES.contains(&name))
}

fn validate_path_name(name: &str) -> Result<(), PackError> {
    let mut components = Path::new(name).components();
    if name != ".dtm"
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
    {
        Ok(())
    } else {
        Err(PackError::InvalidPathName(name.to_owned()))
    }
}

fn ensure_pack_path_round_trip(original: &Path, mapped: &Path) -> Result<(), PackError> {
    let mut restored = PathBuf::new();
    for component in mapped.components() {
        let Component::Normal(name) = component else {
            return Err(PackError::UnrepresentablePath(original.to_path_buf()));
        };
        let Some((kind, name)) = deployment_name(name) else {
            return Err(PackError::UnrepresentablePath(original.to_path_buf()));
        };
        if kind != EntryKind::Symlink {
            return Err(PackError::UnrepresentablePath(original.to_path_buf()));
        }
        restored.push(name);
    }
    if restored != original {
        return Err(PackError::UnrepresentablePath(original.to_path_buf()));
    }
    Ok(())
}

fn validate_existing_package(
    pkgs_dir: &Path,
    package_name: &str,
    paths: &BTreeMap<String, String>,
) -> Result<(), PackError> {
    match fs::symlink_metadata(pkgs_dir.join(package_name)) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            PackagePlan::load(pkgs_dir, package_name, paths).map_err(PackError::Package)?;
            Ok(())
        }
        Ok(_) => Err(PackError::InvalidPackageRoot(pkgs_dir.join(package_name))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(PackError::ReadMetadata {
            path: pkgs_dir.join(package_name),
            source,
        }),
    }
}

fn validate_package_destination(package_root: &Path, destination: &Path) -> Result<(), PackError> {
    let mut current = destination.parent();
    while let Some(path) = current {
        if path == package_root {
            break;
        }
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(PackError::DestinationParentNotDirectory {
                    path: path.to_path_buf(),
                });
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(PackError::ReadMetadata {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
        current = path.parent();
    }
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(PackError::InvalidDestinationType {
            path: destination.to_path_buf(),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(PackError::ReadMetadata {
            path: destination.to_path_buf(),
            source,
        }),
    }
}

fn regular_file_exists(path: &Path) -> Result<bool, PackError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_file() && !metadata.file_type().is_symlink()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(PackError::ReadMetadata {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn create_transaction_directory(pkgs_dir: &Path, package: &str) -> Result<PathBuf, PackError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..100_u32 {
        let path = pkgs_dir.join(format!(
            ".dtm-pack-{package}-{}-{timestamp}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(PackError::CreateDirectory { path, source }),
        }
    }
    Err(PackError::CreateTransactionDirectory)
}

fn verify_sources_unchanged(entries: &[PackEntry]) -> Result<(), PackError> {
    for entry in entries {
        let source_metadata =
            fs::symlink_metadata(&entry.source).map_err(|error| PackError::ReadSource {
                path: entry.source.clone(),
                source: error,
            })?;
        let package_metadata =
            fs::symlink_metadata(&entry.package_path).map_err(|error| PackError::ReadSource {
                path: entry.package_path.clone(),
                source: error,
            })?;
        if !source_metadata.is_file()
            || source_metadata.file_type().is_symlink()
            || !package_metadata.is_file()
            || package_metadata.file_type().is_symlink()
            || !same_permissions(&source_metadata, &package_metadata)
        {
            return Err(PackError::SourceChanged(entry.source.clone()));
        }
        let source = fs::read(&entry.source).map_err(|error| PackError::ReadSource {
            path: entry.source.clone(),
            source: error,
        })?;
        let package = fs::read(&entry.package_path).map_err(|error| PackError::ReadSource {
            path: entry.package_path.clone(),
            source: error,
        })?;
        if source != package {
            return Err(PackError::SourceChanged(entry.source.clone()));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn same_permissions(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    left.permissions().mode() == right.permissions().mode()
}

#[cfg(not(unix))]
fn same_permissions(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.permissions().readonly() == right.permissions().readonly()
}

fn package_management_state(
    plan: &PackagePlan,
    template_values: &BTreeMap<String, String>,
) -> Result<Vec<bool>, PackageError> {
    plan.entries
        .iter()
        .map(|entry| {
            let rendered = match entry.kind {
                EntryKind::Symlink => None,
                EntryKind::Template => Some(crate::package::render_template(
                    &entry.source,
                    template_values,
                )?),
            };
            target_is_current_entry(entry, rendered.as_deref())
        })
        .collect()
}

fn remove_newly_installed(
    plan: &PackagePlan,
    template_values: &BTreeMap<String, String>,
    before: &[bool],
) -> Vec<String> {
    let mut failures = Vec::new();
    for (index, entry) in plan.entries.iter().enumerate().rev() {
        if before.get(index).copied().unwrap_or(false) {
            continue;
        }
        let rendered = match entry.kind {
            EntryKind::Symlink => None,
            EntryKind::Template => {
                match crate::package::render_template(&entry.source, template_values) {
                    Ok(rendered) => Some(rendered),
                    Err(error) => {
                        failures.push(error.to_string());
                        continue;
                    }
                }
            }
        };
        match target_is_current_entry(entry, rendered.as_deref()) {
            Ok(true) => {
                if let Err(error) = fs::remove_file(&entry.target) {
                    failures.push(format!(
                        "could not remove {}: {error}",
                        entry.target.display()
                    ));
                }
            }
            Ok(false) => {}
            Err(error) => failures.push(error.to_string()),
        }
    }
    failures
}

fn restore_sources(entries: &[PackEntry]) -> Vec<String> {
    let mut failures = Vec::new();
    for entry in entries.iter().rev() {
        if let Some(parent) = entry.source.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            failures.push(format!("could not recreate {}: {error}", parent.display()));
            continue;
        }
        match fs::symlink_metadata(&entry.source) {
            Ok(_) => failures.push(format!(
                "could not restore {} because it now exists",
                entry.source.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = fs::copy(&entry.package_path, &entry.source) {
                    failures.push(format!(
                        "could not restore {} from {}: {error}",
                        entry.source.display(),
                        entry.package_path.display()
                    ));
                }
            }
            Err(error) => failures.push(format!(
                "could not inspect {} during rollback: {error}",
                entry.source.display()
            )),
        }
    }
    failures
}

fn rollback_package_files(changes: &[PackageChange]) -> Vec<String> {
    let mut failures = Vec::new();
    for change in changes.iter().rev() {
        if let Err(error) = fs::remove_file(&change.package_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            failures.push(format!(
                "could not remove package file {}: {error}",
                change.package_path.display()
            ));
            continue;
        }
        if let Some(old) = &change.old
            && let Err(error) = fs::rename(old, &change.package_path)
        {
            failures.push(format!(
                "could not restore package file {}: {error}",
                change.package_path.display()
            ));
        }
    }
    remove_empty_package_directories(changes, &mut failures);
    failures
}

fn remove_empty_package_directories(changes: &[PackageChange], failures: &mut Vec<String>) {
    let mut directories = changes
        .iter()
        .filter_map(|change| change.package_path.parent().map(Path::to_path_buf))
        .collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    directories.dedup();
    for directory in directories {
        match fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => failures.push(format!(
                "could not remove rollback directory {}: {error}",
                directory.display()
            )),
        }
    }
}

fn with_rollback(error: PackError, rollback: Vec<String>) -> PackError {
    if rollback.is_empty() {
        error
    } else {
        PackError::Rollback {
            operation: error.to_string(),
            failures: rollback,
        }
    }
}

fn absolute_path(path: &Path, current_dir: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[derive(Debug)]
pub enum PackError {
    Package(PackageError),
    CurrentDirectory(std::io::Error),
    NoInput,
    NoFiles,
    ReadDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    ReadMetadata {
        path: PathBuf,
        source: std::io::Error,
    },
    SymbolicLink {
        path: PathBuf,
    },
    UnsupportedFileType {
        path: PathBuf,
    },
    InputContainsPackage {
        input: PathBuf,
        package: PathBuf,
    },
    NoPathParent {
        path: PathBuf,
    },
    InvalidPathName(String),
    UnrepresentablePath(PathBuf),
    TargetCollision {
        first: PathBuf,
        second: PathBuf,
        package_path: PathBuf,
    },
    DestinationParentNotDirectory {
        path: PathBuf,
    },
    InvalidDestinationType {
        path: PathBuf,
    },
    InvalidPackageRoot(PathBuf),
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    CreateTransactionDirectory,
    CopyFile {
        source_path: PathBuf,
        destination: PathBuf,
        source: std::io::Error,
    },
    MoveFile {
        source_path: PathBuf,
        destination: PathBuf,
        source: std::io::Error,
    },
    ReadSource {
        path: PathBuf,
        source: std::io::Error,
    },
    SourceChanged(PathBuf),
    RemoveSource {
        path: PathBuf,
        source: std::io::Error,
    },
    MappingMismatch {
        source: PathBuf,
        package_path: PathBuf,
    },
    InstallSkipped {
        paths: Vec<PathBuf>,
    },
    CleanupTransaction {
        path: PathBuf,
        source: std::io::Error,
    },
    Rollback {
        operation: String,
        failures: Vec<String>,
    },
}

impl fmt::Display for PackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Package(error) => write!(formatter, "{error}"),
            Self::CurrentDirectory(error) => {
                write!(formatter, "could not read current directory: {error}")
            }
            Self::NoInput => write!(formatter, "pack requires at least one file or directory"),
            Self::NoFiles => write!(formatter, "no files were selected for packing"),
            Self::ReadDirectory { path, source } => write!(
                formatter,
                "could not read directory {}: {source}",
                path.display()
            ),
            Self::ReadMetadata { path, source } => {
                write!(formatter, "could not inspect {}: {source}", path.display())
            }
            Self::SymbolicLink { path } => write!(
                formatter,
                "symbolic links cannot be packed: {}",
                path.display()
            ),
            Self::UnsupportedFileType { path } => write!(
                formatter,
                "unsupported file type cannot be packed: {}",
                path.display()
            ),
            Self::InputContainsPackage { input, package } => write!(
                formatter,
                "input {} contains package destination {}",
                input.display(),
                package.display()
            ),
            Self::NoPathParent { path } => write!(
                formatter,
                "no configured path is a parent of {}",
                path.display()
            ),
            Self::InvalidPathName(name) => {
                write!(
                    formatter,
                    "path name cannot be used as a package directory: {name}"
                )
            }
            Self::UnrepresentablePath(path) => write!(
                formatter,
                "path cannot be represented losslessly by package naming rules: {}",
                path.display()
            ),
            Self::TargetCollision {
                first,
                second,
                package_path,
            } => write!(
                formatter,
                "{} and {} both map to package file {}",
                first.display(),
                second.display(),
                package_path.display()
            ),
            Self::DestinationParentNotDirectory { path } => write!(
                formatter,
                "package destination parent is not a directory: {}",
                path.display()
            ),
            Self::InvalidDestinationType { path } => write!(
                formatter,
                "package destination is not a regular file: {}",
                path.display()
            ),
            Self::InvalidPackageRoot(path) => {
                write!(formatter, "invalid package root: {}", path.display())
            }
            Self::CreateDirectory { path, source } => write!(
                formatter,
                "could not create directory {}: {source}",
                path.display()
            ),
            Self::CreateTransactionDirectory => {
                write!(formatter, "could not allocate a temporary pack directory")
            }
            Self::CopyFile {
                source_path,
                destination,
                source,
            } => write!(
                formatter,
                "could not copy {} to {}: {source}",
                source_path.display(),
                destination.display()
            ),
            Self::MoveFile {
                source_path,
                destination,
                source,
            } => write!(
                formatter,
                "could not move {} to {}: {source}",
                source_path.display(),
                destination.display()
            ),
            Self::ReadSource { path, source } => write!(
                formatter,
                "could not verify source {} after copying: {source}",
                path.display()
            ),
            Self::SourceChanged(path) => write!(
                formatter,
                "source changed while it was being packed: {}",
                path.display()
            ),
            Self::RemoveSource { path, source } => write!(
                formatter,
                "could not remove packed source {}: {source}",
                path.display()
            ),
            Self::MappingMismatch {
                source,
                package_path,
            } => write!(
                formatter,
                "package file {} does not map back to source {}",
                package_path.display(),
                source.display()
            ),
            Self::InstallSkipped { paths } => write!(
                formatter,
                "stow skipped newly packed targets: {}",
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::CleanupTransaction { path, source } => write!(
                formatter,
                "pack succeeded but temporary directory {} could not be removed: {source}",
                path.display()
            ),
            Self::Rollback {
                operation,
                failures,
            } => write!(
                formatter,
                "{operation}; rollback also failed: {}",
                failures.join("; ")
            ),
        }
    }
}

impl Error for PackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Package(error) => Some(error),
            Self::CurrentDirectory(error)
            | Self::ReadDirectory { source: error, .. }
            | Self::ReadMetadata { source: error, .. }
            | Self::CreateDirectory { source: error, .. }
            | Self::CopyFile { source: error, .. }
            | Self::MoveFile { source: error, .. }
            | Self::RemoveSource { source: error, .. }
            | Self::CleanupTransaction { source: error, .. } => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
