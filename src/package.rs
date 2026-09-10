use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

const METADATA_DIRECTORY: &str = ".dtm";

#[derive(Debug, PartialEq, Eq)]
pub struct PackagePlan {
    pub name: String,
    pub root: PathBuf,
    pub entries: Vec<PackageEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEntry {
    pub source: PathBuf,
    pub target: PathBuf,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Symlink,
    Template,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApplyMode {
    #[default]
    Normal,
    SemiForce,
    Force,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub applied: Vec<PackageEntry>,
    pub skipped: Vec<SkippedEntry>,
    pub backups: Vec<BackupEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    pub original: PathBuf,
    pub backup: PathBuf,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RemoveMode {
    #[default]
    Safe,
    SkipUnmanaged,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RemoveReport {
    pub removed: Vec<PackageEntry>,
    pub skipped: Vec<PackageEntry>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub restored: Vec<RestoredEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredEntry {
    pub backup: PathBuf,
    pub target: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    pub entry: PackageEntry,
    pub reason: String,
}

impl PackagePlan {
    pub fn load(
        pkgs_dir: &Path,
        package_name: &str,
        paths: &BTreeMap<String, String>,
    ) -> Result<Self, PackageError> {
        validate_package_name(package_name)?;

        let root = pkgs_dir.join(package_name);
        let metadata =
            fs::symlink_metadata(&root).map_err(|source| PackageError::ReadDirectory {
                path: root.clone(),
                source,
            })?;
        if !metadata.file_type().is_dir() {
            return Err(PackageError::NotDirectory { path: root });
        }
        let root = fs::canonicalize(&root)
            .map_err(|source| PackageError::ReadDirectory { path: root, source })?;

        let mut entries = Vec::new();
        let directories = fs::read_dir(&root).map_err(|source| PackageError::ReadDirectory {
            path: root.clone(),
            source,
        })?;

        for directory in directories {
            let directory = directory.map_err(|source| PackageError::ReadDirectory {
                path: root.clone(),
                source,
            })?;
            let source_path = directory.path();
            let file_name = directory.file_name();
            let file_name = file_name.to_string_lossy();
            let file_type = directory
                .file_type()
                .map_err(|error| PackageError::ReadMetadata {
                    path: source_path.clone(),
                    source: error,
                })?;

            if file_name == METADATA_DIRECTORY {
                if !file_type.is_dir() {
                    return Err(PackageError::MetadataNotDirectory { path: source_path });
                }
                continue;
            }

            if !file_type.is_dir() {
                return Err(PackageError::UnexpectedRootEntry { path: source_path });
            }

            let variable_name = file_name.to_string();
            let destination_root = paths
                .get(file_name.as_ref())
                .map(PathBuf::from)
                .ok_or_else(|| PackageError::UnknownVariableDirectory {
                    path: source_path.clone(),
                    variable: variable_name.clone(),
                })?;
            if !destination_root.is_absolute() {
                return Err(PackageError::RelativeVariablePath {
                    variable: variable_name,
                    value: destination_root,
                });
            }

            scan_directory(&source_path, &destination_root, &mut entries)?;
        }

        entries.sort_by(|left, right| left.target.cmp(&right.target));
        ensure_unique_targets(&entries)?;

        Ok(Self {
            name: package_name.to_owned(),
            root,
            entries,
        })
    }

    pub fn apply(
        &self,
        template_values: &BTreeMap<String, String>,
        mode: ApplyMode,
    ) -> Result<ApplyReport, PackageError> {
        let mut report = ApplyReport::default();
        for entry in &self.entries {
            match apply_entry(entry, template_values, mode, None)? {
                EntryAction::Applied(backup) => {
                    report.applied.push(entry.clone());
                    if let Some(backup) = backup {
                        report.backups.push(backup);
                    }
                }
                EntryAction::Skipped(reason) => report.skipped.push(SkippedEntry {
                    entry: entry.clone(),
                    reason,
                }),
            }
        }
        Ok(report)
    }

    pub fn apply_with_backup(
        &self,
        paths: &BTreeMap<String, String>,
        template_values: &BTreeMap<String, String>,
        mode: ApplyMode,
        backup_dir: &Path,
    ) -> Result<ApplyReport, PackageError> {
        let mut report = ApplyReport::default();

        for entry in &self.entries {
            let backup_path = self.backup_path(entry, paths, backup_dir)?;
            match apply_entry(entry, template_values, mode, Some(&backup_path))? {
                EntryAction::Applied(backup) => {
                    report.applied.push(entry.clone());
                    if let Some(backup) = backup {
                        report.backups.push(backup);
                    }
                }
                EntryAction::Skipped(reason) => report.skipped.push(SkippedEntry {
                    entry: entry.clone(),
                    reason,
                }),
            }
        }

        Ok(report)
    }

    pub fn remove(
        &self,
        template_values: &BTreeMap<String, String>,
        mode: RemoveMode,
    ) -> Result<RemoveReport, PackageError> {
        let mut managed = Vec::new();
        let mut unmanaged = Vec::new();

        for entry in &self.entries {
            let rendered = match entry.kind {
                EntryKind::Symlink => None,
                EntryKind::Template => Some(render_template(&entry.source, template_values)?),
            };
            if target_is_current_entry(entry, rendered.as_deref())? {
                managed.push(entry.clone());
            } else {
                unmanaged.push(entry.clone());
            }
        }

        if mode == RemoveMode::Safe && !unmanaged.is_empty() {
            return Err(PackageError::UnmanagedTargets {
                paths: unmanaged.iter().map(|entry| entry.target.clone()).collect(),
            });
        }

        for entry in &managed {
            remove_target(&entry.target)?;
        }

        Ok(RemoveReport {
            removed: managed,
            skipped: unmanaged,
        })
    }

    pub fn restore(
        &self,
        paths: &BTreeMap<String, String>,
        template_values: &BTreeMap<String, String>,
        backup_dir: &Path,
    ) -> Result<RestoreReport, PackageError> {
        let package_backup = backup_dir.join(&self.name);
        let metadata = fs::symlink_metadata(&package_backup).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                PackageError::BackupPackageNotFound {
                    path: package_backup.clone(),
                }
            } else {
                PackageError::ReadMetadata {
                    path: package_backup.clone(),
                    source,
                }
            }
        })?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(PackageError::InvalidBackupEntry {
                path: package_backup,
            });
        }

        let mut pending = Vec::new();
        let mut directories = vec![package_backup.clone()];
        let mut targets = BTreeSet::new();
        let variable_directories = sorted_directory_entries(&package_backup)?;

        for variable_directory in variable_directories {
            let path = variable_directory.path();
            let file_type =
                variable_directory
                    .file_type()
                    .map_err(|source| PackageError::ReadMetadata {
                        path: path.clone(),
                        source,
                    })?;
            if !file_type.is_dir() || file_type.is_symlink() {
                return Err(PackageError::InvalidBackupEntry { path });
            }

            let variable = variable_directory
                .file_name()
                .into_string()
                .map_err(|_| PackageError::InvalidBackupEntry { path: path.clone() })?;
            let target_root = paths.get(&variable).map(PathBuf::from).ok_or_else(|| {
                PackageError::UnknownBackupVariable {
                    path: path.clone(),
                    variable: variable.clone(),
                }
            })?;
            if !target_root.is_absolute() {
                return Err(PackageError::RelativeVariablePath {
                    variable,
                    value: target_root,
                });
            }

            directories.push(path.clone());
            scan_restore_directory(
                &path,
                &path,
                &target_root,
                self,
                &mut pending,
                &mut directories,
                &mut targets,
            )?;
        }

        if pending.is_empty() {
            return Err(PackageError::BackupPackageEmpty {
                path: package_backup,
            });
        }

        self.remove(template_values, RemoveMode::Safe)?;

        let mut report = RestoreReport::default();
        for restore in pending {
            fs::rename(&restore.backup, &restore.target).map_err(|source| {
                PackageError::RestoreBackup {
                    backup: restore.backup.clone(),
                    target: restore.target.clone(),
                    source,
                }
            })?;
            report.restored.push(restore);
        }

        directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        directories.dedup();
        for directory in directories {
            fs::remove_dir(&directory).map_err(|source| PackageError::RemoveBackupDirectory {
                path: directory,
                source,
            })?;
        }

        Ok(report)
    }

    fn backup_path(
        &self,
        entry: &PackageEntry,
        paths: &BTreeMap<String, String>,
        backup_dir: &Path,
    ) -> Result<PathBuf, PackageError> {
        let (variable, target_relative) =
            nearest_path_target(&entry.target, paths).ok_or_else(|| {
                PackageError::InvalidBackupTarget {
                    path: entry.target.clone(),
                }
            })?;

        Ok(backup_dir
            .join(&self.name)
            .join(variable)
            .join(reverse_backup_path(&target_relative)?))
    }
}

#[derive(Debug)]
enum EntryAction {
    Applied(Option<BackupEntry>),
    Skipped(String),
}

fn apply_entry(
    entry: &PackageEntry,
    template_values: &BTreeMap<String, String>,
    mode: ApplyMode,
    backup_dir: Option<&Path>,
) -> Result<EntryAction, PackageError> {
    let rendered = match entry.kind {
        EntryKind::Symlink => None,
        EntryKind::Template => Some(render_template(&entry.source, template_values)?),
    };

    let target_metadata = match fs::symlink_metadata(&entry.target) {
        Ok(metadata) => Some(metadata),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(PackageError::ReadMetadata {
                path: entry.target.clone(),
                source,
            });
        }
    };

    let mut backup = None;
    if let Some(metadata) = target_metadata {
        if metadata.file_type().is_dir() {
            return Err(PackageError::TargetIsDirectory {
                path: entry.target.clone(),
            });
        }

        if target_is_current_entry(entry, rendered.as_deref())? {
            return Ok(EntryAction::Skipped("already up to date".to_owned()));
        }

        let should_replace = match mode {
            ApplyMode::Normal => false,
            ApplyMode::SemiForce => metadata.file_type().is_symlink(),
            ApplyMode::Force => true,
        };

        if !should_replace {
            return Ok(EntryAction::Skipped("target already exists".to_owned()));
        }

        if let Some(backup_dir) = backup_dir {
            backup = Some(move_target_to_backup(&entry.target, backup_dir)?);
        } else {
            remove_target(&entry.target)?;
        }
    }

    if let Some(parent) = entry.target.parent() {
        fs::create_dir_all(parent).map_err(|source| PackageError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    match rendered {
        Some(contents) => {
            fs::write(&entry.target, contents).map_err(|source| PackageError::WriteFile {
                path: entry.target.clone(),
                source,
            })?
        }
        None => create_symlink(&entry.source, &entry.target)?,
    }

    Ok(EntryAction::Applied(backup))
}

pub(crate) fn target_is_current_entry(
    entry: &PackageEntry,
    rendered: Option<&str>,
) -> Result<bool, PackageError> {
    let metadata = match fs::symlink_metadata(&entry.target) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(PackageError::ReadMetadata {
                path: entry.target.clone(),
                source,
            });
        }
    };

    match entry.kind {
        EntryKind::Symlink => {
            if !metadata.file_type().is_symlink() {
                return Ok(false);
            }
            let target = fs::read_link(&entry.target).map_err(|source| PackageError::ReadLink {
                path: entry.target.clone(),
                source,
            })?;
            Ok(target == entry.source)
        }
        EntryKind::Template => {
            let Some(rendered) = rendered else {
                return Ok(false);
            };
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Ok(false);
            }
            let existing = fs::read(&entry.target).map_err(|source| PackageError::ReadFile {
                path: entry.target.clone(),
                source,
            })?;
            Ok(existing == rendered.as_bytes())
        }
    }
}

fn remove_target(path: &Path) -> Result<(), PackageError> {
    fs::remove_file(path).map_err(|source| PackageError::RemoveTarget {
        path: path.to_path_buf(),
        source,
    })
}

fn move_target_to_backup(target: &Path, backup: &Path) -> Result<BackupEntry, PackageError> {
    if path_exists(backup)? {
        return Err(PackageError::BackupAlreadyExists {
            path: backup.to_path_buf(),
        });
    }
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent).map_err(|source| PackageError::CreateBackupDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::rename(target, backup).map_err(|source| PackageError::MoveToBackup {
        source_path: target.to_path_buf(),
        backup: backup.to_path_buf(),
        source,
    })?;

    Ok(BackupEntry {
        original: target.to_path_buf(),
        backup: backup.to_path_buf(),
    })
}

fn path_exists(path: &Path) -> Result<bool, PackageError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(PackageError::ReadMetadata {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
fn create_symlink(source: &Path, target: &Path) -> Result<(), PackageError> {
    std::os::unix::fs::symlink(source, target).map_err(|source_error| PackageError::CreateLink {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        source_error,
    })
}

#[cfg(not(unix))]
fn create_symlink(_source: &Path, _target: &Path) -> Result<(), PackageError> {
    Err(PackageError::UnsupportedPlatform)
}

pub(crate) fn nearest_path_target<'a>(
    target: &Path,
    paths: &'a BTreeMap<String, String>,
) -> Option<(&'a str, PathBuf)> {
    paths
        .iter()
        .filter_map(|(name, value)| {
            let root = Path::new(value);
            if !root.is_absolute() {
                return None;
            }
            let relative = target.strip_prefix(root).ok()?;
            Some((
                name.as_str(),
                relative.to_path_buf(),
                root.components().count(),
            ))
        })
        .max_by(|left, right| left.2.cmp(&right.2).then_with(|| right.0.cmp(left.0)))
        .map(|(name, relative, _)| (name, relative))
}

pub(crate) fn reverse_backup_path(path: &Path) -> Result<PathBuf, PackageError> {
    map_backup_path(path, true)
}

fn restore_backup_path(path: &Path) -> Result<PathBuf, PackageError> {
    map_backup_path(path, false)
}

fn map_backup_path(path: &Path, reverse: bool) -> Result<PathBuf, PackageError> {
    let mut mapped = PathBuf::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(PackageError::InvalidBackupTarget {
                path: path.to_path_buf(),
            });
        };
        let name = name
            .to_str()
            .ok_or_else(|| PackageError::InvalidBackupTarget {
                path: path.to_path_buf(),
            })?;
        let mapped_name = if reverse {
            name.strip_prefix('.')
                .filter(|suffix| !suffix.is_empty())
                .map(|suffix| format!("dot-{suffix}"))
                .unwrap_or_else(|| name.to_owned())
        } else {
            name.strip_prefix("dot-")
                .filter(|suffix| !suffix.is_empty())
                .map(|suffix| format!(".{suffix}"))
                .unwrap_or_else(|| name.to_owned())
        };
        mapped.push(mapped_name);
    }

    if mapped.as_os_str().is_empty() {
        return Err(PackageError::InvalidBackupTarget {
            path: path.to_path_buf(),
        });
    }
    Ok(mapped)
}

fn sorted_directory_entries(path: &Path) -> Result<Vec<fs::DirEntry>, PackageError> {
    let entries = fs::read_dir(path).map_err(|source| PackageError::ReadDirectory {
        path: path.to_path_buf(),
        source,
    })?;
    let mut entries = entries
        .map(|entry| {
            entry.map_err(|source| PackageError::ReadDirectory {
                path: path.to_path_buf(),
                source,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

#[allow(clippy::too_many_arguments)]
fn scan_restore_directory(
    directory: &Path,
    variable_backup_root: &Path,
    target_root: &Path,
    plan: &PackagePlan,
    pending: &mut Vec<RestoredEntry>,
    directories: &mut Vec<PathBuf>,
    targets: &mut BTreeSet<PathBuf>,
) -> Result<(), PackageError> {
    for backup_entry in sorted_directory_entries(directory)? {
        let backup = backup_entry.path();
        let file_type = backup_entry
            .file_type()
            .map_err(|source| PackageError::ReadMetadata {
                path: backup.clone(),
                source,
            })?;

        if file_type.is_dir() && !file_type.is_symlink() {
            directories.push(backup.clone());
            scan_restore_directory(
                &backup,
                variable_backup_root,
                target_root,
                plan,
                pending,
                directories,
                targets,
            )?;
            continue;
        }
        if !file_type.is_file() && !file_type.is_symlink() {
            return Err(PackageError::InvalidBackupEntry { path: backup });
        }

        let relative = backup.strip_prefix(variable_backup_root).map_err(|_| {
            PackageError::InvalidBackupEntry {
                path: backup.clone(),
            }
        })?;
        let target_relative = restore_backup_path(relative)?;
        if reverse_backup_path(&target_relative)? != relative {
            return Err(PackageError::InvalidBackupEntry { path: backup });
        }
        let target = target_root.join(target_relative);
        if !targets.insert(target.clone()) {
            return Err(PackageError::DuplicateRestoreTarget { path: target });
        }
        plan.entries
            .iter()
            .find(|entry| entry.target == target)
            .ok_or_else(|| PackageError::BackupTargetNotInPackage {
                backup: backup.clone(),
                target: target.clone(),
            })?;

        pending.push(RestoredEntry { backup, target });
    }
    Ok(())
}

pub(crate) fn validate_package_name(name: &str) -> Result<(), PackageError> {
    if name.is_empty()
        || name.contains(std::path::MAIN_SEPARATOR)
        || Path::new(name).components().any(|component| {
            matches!(
                component,
                Component::RootDir | Component::CurDir | Component::ParentDir
            )
        })
    {
        return Err(PackageError::InvalidName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn scan_directory(
    source_directory: &Path,
    target_directory: &Path,
    entries: &mut Vec<PackageEntry>,
) -> Result<(), PackageError> {
    let files = fs::read_dir(source_directory).map_err(|source| PackageError::ReadDirectory {
        path: source_directory.to_path_buf(),
        source,
    })?;

    for file in files {
        let file = file.map_err(|source| PackageError::ReadDirectory {
            path: source_directory.to_path_buf(),
            source,
        })?;
        let source_path = file.path();
        let file_name = file.file_name();
        let (kind, target_name) =
            deployment_name(&file_name).ok_or_else(|| PackageError::InvalidFileName {
                path: source_path.clone(),
            })?;
        let target = target_directory.join(target_name);
        let file_type = file
            .file_type()
            .map_err(|error| PackageError::ReadMetadata {
                path: source_path.clone(),
                source: error,
            })?;

        if file_type.is_symlink() {
            return Err(PackageError::SymbolicLink { path: source_path });
        }
        if file_type.is_dir() {
            scan_directory(&source_path, &target, entries)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(PackageError::UnsupportedFileType { path: source_path });
        }

        entries.push(PackageEntry {
            source: source_path,
            target,
            kind,
        });
    }

    Ok(())
}

pub(crate) fn deployment_name(name: &std::ffi::OsStr) -> Option<(EntryKind, std::ffi::OsString)> {
    let name = name.to_str()?;
    let (kind, name) = if let Some(marker_position) = name.rfind(".tmpl") {
        if name == ".tmpl" {
            (EntryKind::Symlink, name.to_owned())
        } else {
            let mut name_without_marker = String::with_capacity(name.len() - ".tmpl".len());
            name_without_marker.push_str(&name[..marker_position]);
            name_without_marker.push_str(&name[marker_position + ".tmpl".len()..]);
            (EntryKind::Template, name_without_marker)
        }
    } else {
        (EntryKind::Symlink, name.to_owned())
    };
    let name = name
        .strip_prefix("dot-")
        .filter(|suffix| !suffix.is_empty())
        .map(|suffix| format!(".{suffix}"))
        .unwrap_or(name);

    Some((kind, name.into()))
}

fn ensure_unique_targets(entries: &[PackageEntry]) -> Result<(), PackageError> {
    for pair in entries.windows(2) {
        if pair[0].target == pair[1].target {
            return Err(PackageError::TargetCollision {
                first: pair[0].source.clone(),
                second: pair[1].source.clone(),
                target: pair[0].target.clone(),
            });
        }
    }
    Ok(())
}

pub fn render_template(
    source: &Path,
    template_values: &BTreeMap<String, String>,
) -> Result<String, PackageError> {
    let contents = fs::read_to_string(source).map_err(|source_error| PackageError::ReadFile {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let mut rendered = String::with_capacity(contents.len());
    let mut remaining = contents.as_str();

    while let Some(start) = remaining.find("{=") {
        rendered.push_str(&remaining[..start]);
        let marker = &remaining[start + 2..];
        let Some(end) = marker.find("=}") else {
            rendered.push_str(&remaining[start..]);
            remaining = "";
            break;
        };

        let name = &marker[..end];
        if name.is_empty() || name.contains(['\r', '\n']) {
            rendered.push_str(&remaining[start..start + 2 + end + 2]);
        } else {
            let name = name.trim();
            let value =
                template_values
                    .get(name)
                    .ok_or_else(|| PackageError::UnknownTemplateVariable {
                        path: source.to_path_buf(),
                        variable: name.to_owned(),
                    })?;
            rendered.push_str(value);
        }
        remaining = &marker[end + 2..];
    }

    rendered.push_str(remaining);

    Ok(rendered)
}

#[derive(Debug)]
pub enum PackageError {
    InvalidName {
        name: String,
    },
    ReadDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    ReadMetadata {
        path: PathBuf,
        source: std::io::Error,
    },
    NotDirectory {
        path: PathBuf,
    },
    MetadataNotDirectory {
        path: PathBuf,
    },
    UnexpectedRootEntry {
        path: PathBuf,
    },
    UnknownVariableDirectory {
        path: PathBuf,
        variable: String,
    },
    RelativeVariablePath {
        variable: String,
        value: PathBuf,
    },
    SymbolicLink {
        path: PathBuf,
    },
    UnsupportedFileType {
        path: PathBuf,
    },
    InvalidFileName {
        path: PathBuf,
    },
    TargetCollision {
        first: PathBuf,
        second: PathBuf,
        target: PathBuf,
    },
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },
    ReadLink {
        path: PathBuf,
        source: std::io::Error,
    },
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },
    RemoveTarget {
        path: PathBuf,
        source: std::io::Error,
    },
    CreateBackupDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    MoveToBackup {
        source_path: PathBuf,
        backup: PathBuf,
        source: std::io::Error,
    },
    InvalidBackupTarget {
        path: PathBuf,
    },
    BackupAlreadyExists {
        path: PathBuf,
    },
    BackupPackageNotFound {
        path: PathBuf,
    },
    BackupPackageEmpty {
        path: PathBuf,
    },
    InvalidBackupEntry {
        path: PathBuf,
    },
    UnknownBackupVariable {
        path: PathBuf,
        variable: String,
    },
    BackupTargetNotInPackage {
        backup: PathBuf,
        target: PathBuf,
    },
    DuplicateRestoreTarget {
        path: PathBuf,
    },
    UnmanagedTargets {
        paths: Vec<PathBuf>,
    },
    RestoreBackup {
        backup: PathBuf,
        target: PathBuf,
        source: std::io::Error,
    },
    RemoveBackupDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    CreateLink {
        source: PathBuf,
        target: PathBuf,
        source_error: std::io::Error,
    },
    TargetIsDirectory {
        path: PathBuf,
    },
    UnknownTemplateVariable {
        path: PathBuf,
        variable: String,
    },
    #[cfg(not(unix))]
    UnsupportedPlatform,
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { name } => write!(formatter, "invalid package name '{name}'"),
            Self::ReadDirectory { path, source } => write!(
                formatter,
                "could not read directory {}: {source}",
                path.display()
            ),
            Self::ReadMetadata { path, source } => {
                write!(formatter, "could not inspect {}: {source}", path.display())
            }
            Self::NotDirectory { path } => write!(
                formatter,
                "package path is not a directory: {}",
                path.display()
            ),
            Self::MetadataNotDirectory { path } => write!(
                formatter,
                "package metadata path must be a directory: {}",
                path.display()
            ),
            Self::UnexpectedRootEntry { path } => write!(
                formatter,
                "package root only allows path directories and {METADATA_DIRECTORY}: {}",
                path.display()
            ),
            Self::UnknownVariableDirectory { path, variable } => write!(
                formatter,
                "package directory {} references unknown path '{variable}'",
                path.display()
            ),
            Self::RelativeVariablePath { variable, value } => write!(
                formatter,
                "path '{variable}' must resolve to an absolute path, got {}",
                value.display()
            ),
            Self::SymbolicLink { path } => write!(
                formatter,
                "symbolic links are not supported in package sources: {}",
                path.display()
            ),
            Self::UnsupportedFileType { path } => write!(
                formatter,
                "unsupported file type in package source: {}",
                path.display()
            ),
            Self::InvalidFileName { path } => {
                write!(formatter, "invalid template filename: {}", path.display())
            }
            Self::TargetCollision {
                first,
                second,
                target,
            } => write!(
                formatter,
                "package files {} and {} both map to {}",
                first.display(),
                second.display(),
                target.display()
            ),
            Self::ReadFile { path, source } => {
                write!(
                    formatter,
                    "could not read file {}: {source}",
                    path.display()
                )
            }
            Self::ReadLink { path, source } => {
                write!(
                    formatter,
                    "could not read link {}: {source}",
                    path.display()
                )
            }
            Self::CreateDirectory { path, source } => write!(
                formatter,
                "could not create directory {}: {source}",
                path.display()
            ),
            Self::WriteFile { path, source } => {
                write!(
                    formatter,
                    "could not write file {}: {source}",
                    path.display()
                )
            }
            Self::RemoveTarget { path, source } => {
                write!(
                    formatter,
                    "could not remove target {}: {source}",
                    path.display()
                )
            }
            Self::CreateBackupDirectory { path, source } => write!(
                formatter,
                "could not create backup directory {}: {source}",
                path.display()
            ),
            Self::MoveToBackup {
                source_path,
                backup,
                source,
            } => write!(
                formatter,
                "could not move {} to backup {}: {source}",
                source_path.display(),
                backup.display()
            ),
            Self::InvalidBackupTarget { path } => write!(
                formatter,
                "could not derive a backup path for {}",
                path.display()
            ),
            Self::BackupAlreadyExists { path } => write!(
                formatter,
                "backup already exists, restore it before stowing again: {}",
                path.display()
            ),
            Self::BackupPackageNotFound { path } => {
                write!(
                    formatter,
                    "package backup does not exist: {}",
                    path.display()
                )
            }
            Self::BackupPackageEmpty { path } => {
                write!(formatter, "package backup is empty: {}", path.display())
            }
            Self::InvalidBackupEntry { path } => write!(
                formatter,
                "invalid file or directory in package backup: {}",
                path.display()
            ),
            Self::UnknownBackupVariable { path, variable } => write!(
                formatter,
                "backup directory {} references unknown path '{variable}'",
                path.display()
            ),
            Self::BackupTargetNotInPackage { backup, target } => write!(
                formatter,
                "backup {} maps to {}, which is not managed by this package",
                backup.display(),
                target.display()
            ),
            Self::DuplicateRestoreTarget { path } => write!(
                formatter,
                "multiple backup files map to restore target {}",
                path.display()
            ),
            Self::UnmanagedTargets { paths } => write!(
                formatter,
                "package has targets not managed by dtm: {}",
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::RestoreBackup {
                backup,
                target,
                source,
            } => write!(
                formatter,
                "could not restore backup {} to {}: {source}",
                backup.display(),
                target.display()
            ),
            Self::RemoveBackupDirectory { path, source } => write!(
                formatter,
                "could not remove restored backup directory {}: {source}",
                path.display()
            ),
            Self::CreateLink {
                source,
                target,
                source_error,
            } => write!(
                formatter,
                "could not link {} to {}: {source_error}",
                source.display(),
                target.display()
            ),
            Self::TargetIsDirectory { path } => write!(
                formatter,
                "target is a directory, refusing to replace it: {}",
                path.display()
            ),
            Self::UnknownTemplateVariable { path, variable } => write!(
                formatter,
                "template {} references unknown variable '{variable}'",
                path.display()
            ),
            #[cfg(not(unix))]
            Self::UnsupportedPlatform => {
                write!(
                    formatter,
                    "symbolic links are not supported on this platform"
                )
            }
        }
    }
}

impl Error for PackageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadDirectory { source, .. }
            | Self::ReadMetadata { source, .. }
            | Self::ReadFile { source, .. }
            | Self::ReadLink { source, .. }
            | Self::CreateDirectory { source, .. }
            | Self::WriteFile { source, .. }
            | Self::RemoveTarget { source, .. }
            | Self::CreateBackupDirectory { source, .. }
            | Self::MoveToBackup { source, .. }
            | Self::RestoreBackup { source, .. }
            | Self::RemoveBackupDirectory { source, .. }
            | Self::CreateLink {
                source_error: source,
                ..
            } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
