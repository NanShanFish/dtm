use std::collections::BTreeMap;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    Normal,
    SemiForce,
    Force,
}

impl Default for ApplyMode {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub applied: Vec<PackageEntry>,
    pub skipped: Vec<SkippedEntry>,
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
        variables: &BTreeMap<String, String>,
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
            let destination_root = variables
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
        variables: &BTreeMap<String, String>,
        mode: ApplyMode,
    ) -> Result<ApplyReport, PackageError> {
        let mut report = ApplyReport::default();

        for entry in &self.entries {
            match apply_entry(entry, variables, mode)? {
                EntryAction::Applied => report.applied.push(entry.clone()),
                EntryAction::Skipped(reason) => report.skipped.push(SkippedEntry {
                    entry: entry.clone(),
                    reason,
                }),
            }
        }

        Ok(report)
    }
}

#[derive(Debug)]
enum EntryAction {
    Applied,
    Skipped(String),
}

fn apply_entry(
    entry: &PackageEntry,
    variables: &BTreeMap<String, String>,
    mode: ApplyMode,
) -> Result<EntryAction, PackageError> {
    let rendered = match entry.kind {
        EntryKind::Symlink => None,
        EntryKind::Template => Some(render_template(&entry.source, variables)?),
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

    if let Some(metadata) = target_metadata {
        if metadata.file_type().is_dir() {
            return Err(PackageError::TargetIsDirectory {
                path: entry.target.clone(),
            });
        }

        if target_is_current_entry(entry, rendered.as_deref())? {
            return Ok(EntryAction::Skipped("already up to date".to_owned()));
        }

        let should_remove = match mode {
            ApplyMode::Normal => false,
            ApplyMode::SemiForce => metadata.file_type().is_symlink(),
            ApplyMode::Force => true,
        };

        if !should_remove {
            return Ok(EntryAction::Skipped("target already exists".to_owned()));
        }

        remove_target(&entry.target, metadata.file_type().is_symlink())?;
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

    Ok(EntryAction::Applied)
}

fn target_is_current_entry(
    entry: &PackageEntry,
    rendered: Option<&str>,
) -> Result<bool, PackageError> {
    match entry.kind {
        EntryKind::Symlink => {
            if !entry.target.is_symlink() {
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
            if entry.target.is_symlink() {
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

fn remove_target(path: &Path, is_symlink: bool) -> Result<(), PackageError> {
    let result = if is_symlink {
        fs::remove_file(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|source| PackageError::RemoveTarget {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn create_symlink(source: &Path, target: &Path) -> Result<(), PackageError> {
    std::os::unix::fs::symlink(source, target).map_err(|source_error| PackageError::CreateLink {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        source_error,
    })
}

fn validate_package_name(name: &str) -> Result<(), PackageError> {
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

fn deployment_name(name: &std::ffi::OsStr) -> Option<(EntryKind, std::ffi::OsString)> {
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
    variables: &BTreeMap<String, String>,
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
                variables
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
                "package root only allows variable directories and {METADATA_DIRECTORY}: {}",
                path.display()
            ),
            Self::UnknownVariableDirectory { path, variable } => write!(
                formatter,
                "package directory {} references unknown variable '{variable}'",
                path.display()
            ),
            Self::RelativeVariablePath { variable, value } => write!(
                formatter,
                "variable '{variable}' must resolve to an absolute path, got {}",
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
            | Self::CreateLink {
                source_error: source,
                ..
            } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reads_regular_and_template_files_from_a_package() {
        let fixture = TemporaryDirectory::new("package-files");
        let pkgs_dir = fixture.path();
        let package = pkgs_dir.join("tmux");
        fs::create_dir_all(package.join(".dtm/hooks")).expect("create metadata");
        fs::create_dir_all(package.join("config_home/tmux")).expect("create package files");
        fs::write(package.join(".dtm/config.yaml"), "hooks: {}\n").expect("write metadata");
        fs::write(package.join(".dtm/hooks/post-install.sh"), "#!/bin/sh\n").expect("write hook");
        fs::write(
            package.join("config_home/tmux/tmux.conf"),
            "set -g status on\n",
        )
        .expect("write regular file");

        let variables =
            BTreeMap::from([("config_home".to_owned(), "/home/tester/.config".to_owned())]);
        let plan = PackagePlan::load(pkgs_dir, "tmux", &variables).expect("load package");

        assert_eq!(plan.entries.len(), 1);
        assert_eq!(
            plan.entries[0],
            PackageEntry {
                source: package.join("config_home/tmux/tmux.conf"),
                target: PathBuf::from("/home/tester/.config/tmux/tmux.conf"),
                kind: EntryKind::Symlink,
            }
        );
    }

    #[test]
    fn maps_dot_prefixed_files_to_hidden_targets() {
        let fixture = TemporaryDirectory::new("hidden-file");
        let pkgs_dir = fixture.path();
        let package = pkgs_dir.join("git");
        fs::create_dir_all(package.join("home")).expect("create package files");
        fs::write(package.join("home/dot-gitconfig"), "[user]\n").expect("write file");

        let variables = BTreeMap::from([("home".to_owned(), "/home/tester".to_owned())]);
        let plan = PackagePlan::load(pkgs_dir, "git", &variables).expect("load package");

        assert_eq!(
            plan.entries,
            vec![PackageEntry {
                source: package.join("home/dot-gitconfig"),
                target: PathBuf::from("/home/tester/.gitconfig"),
                kind: EntryKind::Symlink,
            }]
        );
    }

    #[test]
    fn reads_template_files_and_removes_the_template_marker() {
        let fixture = TemporaryDirectory::new("template-file");
        let pkgs_dir = fixture.path();
        let package = pkgs_dir.join("tmux");
        fs::create_dir_all(package.join("config_home/tmux")).expect("create package files");
        fs::write(
            package.join("config_home/tmux/tmux.tmpl.conf"),
            "set -g status {=enabled=}\n",
        )
        .expect("write template file");

        let variables =
            BTreeMap::from([("config_home".to_owned(), "/home/tester/.config".to_owned())]);
        let plan = PackagePlan::load(pkgs_dir, "tmux", &variables).expect("load package");

        assert_eq!(
            plan.entries,
            vec![PackageEntry {
                source: package.join("config_home/tmux/tmux.tmpl.conf"),
                target: PathBuf::from("/home/tester/.config/tmux/tmux.conf"),
                kind: EntryKind::Template,
            }]
        );
    }

    #[test]
    fn classifies_deployment_names_when_removing_the_last_template_marker() {
        assert_eq!(
            deployment_name(std::ffi::OsStr::new("dot-gitconfig")),
            Some((EntryKind::Symlink, ".gitconfig".into()))
        );
        assert_eq!(
            deployment_name(std::ffi::OsStr::new("dot-gitconfig.tmpl")),
            Some((EntryKind::Template, ".gitconfig".into()))
        );
        assert_eq!(
            deployment_name(std::ffi::OsStr::new("tmux.conf.tmpl")),
            Some((EntryKind::Template, "tmux.conf".into()))
        );
        assert_eq!(
            deployment_name(std::ffi::OsStr::new("tmux.tmpl.conf")),
            Some((EntryKind::Template, "tmux.conf".into()))
        );
        assert_eq!(
            deployment_name(std::ffi::OsStr::new("app.tmpl.backup.tmpl")),
            Some((EntryKind::Template, "app.tmpl.backup".into()))
        );
    }

    #[test]
    fn rejects_unknown_variable_directory() {
        let fixture = TemporaryDirectory::new("unknown-variable");
        fs::create_dir_all(fixture.path().join("pkg/missing/file")).expect("create package");

        let error = PackagePlan::load(fixture.path(), "pkg", &BTreeMap::new())
            .expect_err("unknown variable");

        assert!(
            matches!(error, PackageError::UnknownVariableDirectory { variable, .. } if variable == "missing")
        );
    }

    #[test]
    fn rejects_files_at_package_root() {
        let fixture = TemporaryDirectory::new("root-file");
        let package = fixture.path().join("pkg");
        fs::create_dir_all(&package).expect("create package");
        fs::write(package.join("script.sh"), "#!/bin/sh\n").expect("write file");

        let error =
            PackagePlan::load(fixture.path(), "pkg", &BTreeMap::new()).expect_err("root file");

        assert!(matches!(error, PackageError::UnexpectedRootEntry { .. }));
    }

    #[test]
    fn rejects_duplicate_targets_created_by_template_name() {
        let fixture = TemporaryDirectory::new("target-collision");
        let package = fixture.path().join("pkg/config/file");
        fs::create_dir_all(&package).expect("create package");
        fs::write(package.join("app.conf"), "plain\n").expect("write file");
        fs::write(package.join("app.conf.tmpl"), "template\n").expect("write file");

        let variables = BTreeMap::from([("config".to_owned(), "/tmp/config".to_owned())]);
        let error =
            PackagePlan::load(fixture.path(), "pkg", &variables).expect_err("target collision");

        assert!(matches!(error, PackageError::TargetCollision { .. }));
    }

    #[test]
    fn applies_a_symlink_and_a_template() {
        let fixture = TemporaryDirectory::new("apply");
        let pkgs_dir = fixture.path().join("dotfiles");
        let package = pkgs_dir.join("pkg/home");
        let target_home = fixture.path().join("target");
        fs::create_dir_all(&package).expect("create package");
        fs::write(package.join("dot-config"), "linked\n").expect("write source");
        fs::write(package.join("dot-settings.conf.tmpl"), "home={=home=}\n")
            .expect("write template");

        let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
        let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
        let report = plan
            .apply(&variables, ApplyMode::Normal)
            .expect("apply package");

        assert_eq!(report.applied.len(), 2);
        assert_eq!(
            fs::read_link(target_home.join(".config")).unwrap(),
            package.join("dot-config")
        );
        assert_eq!(
            fs::read_to_string(target_home.join(".settings.conf")).expect("read template"),
            format!("home={}\n", target_home.display())
        );
    }

    #[test]
    fn normal_mode_skips_different_targets_and_semi_force_replaces_only_links() {
        let fixture = TemporaryDirectory::new("modes");
        let pkgs_dir = fixture.path().join("dotfiles");
        let package = pkgs_dir.join("pkg/home");
        let target_home = fixture.path().join("target");
        fs::create_dir_all(&package).expect("create package");
        fs::write(package.join("dot-config"), "source\n").expect("write source");
        fs::create_dir_all(&target_home).expect("create target");
        fs::write(target_home.join(".config"), "existing\n").expect("write target");

        let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
        let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
        let report = plan
            .apply(&variables, ApplyMode::Normal)
            .expect("normal apply");
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(
            fs::read_to_string(target_home.join(".config")).unwrap(),
            "existing\n"
        );

        let report = plan
            .apply(&variables, ApplyMode::SemiForce)
            .expect("semi-force apply");
        assert_eq!(report.applied.len(), 0);
        assert_eq!(report.skipped.len(), 1);

        fs::remove_file(target_home.join(".config")).expect("remove ordinary target");
        #[cfg(unix)]
        std::os::unix::fs::symlink(package.join("dot-config"), target_home.join(".config"))
            .expect("create existing link");
        let report = plan
            .apply(&variables, ApplyMode::SemiForce)
            .expect("semi-force link apply");
        assert_eq!(report.applied.len(), 0);
        assert_eq!(report.skipped[0].reason, "already up to date");
        assert_eq!(
            fs::read_link(target_home.join(".config")).unwrap(),
            package.join("dot-config")
        );
    }

    #[test]
    fn force_replaces_an_existing_regular_file() {
        let fixture = TemporaryDirectory::new("force");
        let pkgs_dir = fixture.path().join("dotfiles");
        let package = pkgs_dir.join("pkg/home");
        let target_home = fixture.path().join("target");
        fs::create_dir_all(&package).expect("create package");
        fs::write(package.join("dot-config"), "source\n").expect("write source");
        fs::create_dir_all(&target_home).expect("create target");
        fs::write(target_home.join(".config"), "existing\n").expect("write target");

        let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
        let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
        let report = plan
            .apply(&variables, ApplyMode::Force)
            .expect("force apply");

        assert_eq!(report.applied.len(), 1);
        assert_eq!(
            fs::read_link(target_home.join(".config")).unwrap(),
            package.join("dot-config")
        );
    }

    #[test]
    fn template_compares_rendered_content_and_force_replaces_different_file() {
        let fixture = TemporaryDirectory::new("template-conflict");
        let pkgs_dir = fixture.path().join("dotfiles");
        let package = pkgs_dir.join("pkg/home");
        let target_home = fixture.path().join("target");
        fs::create_dir_all(&package).expect("create package");
        fs::write(package.join("settings.tmpl"), "name={=name=}\n").expect("write template");

        let variables = BTreeMap::from([
            ("home".to_owned(), target_home.display().to_string()),
            ("name".to_owned(), "dtm".to_owned()),
        ]);
        let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
        plan.apply(&variables, ApplyMode::Normal)
            .expect("first apply");

        let report = plan
            .apply(&variables, ApplyMode::Normal)
            .expect("second apply");
        assert_eq!(report.applied.len(), 0);
        assert_eq!(report.skipped[0].reason, "already up to date");

        fs::write(target_home.join("settings"), "changed\n").expect("change generated file");
        let report = plan
            .apply(&variables, ApplyMode::Normal)
            .expect("normal apply");
        assert_eq!(report.skipped[0].reason, "target already exists");

        let report = plan
            .apply(&variables, ApplyMode::Force)
            .expect("force apply");
        assert_eq!(report.applied.len(), 1);
        assert_eq!(
            fs::read_to_string(target_home.join("settings")).expect("read generated file"),
            "name=dtm\n"
        );
    }

    #[test]
    fn template_rejects_unknown_variables_and_preserves_unclosed_or_multiline_markers() {
        let fixture = TemporaryDirectory::new("template-rendering");
        let source = fixture.path().join("source.tmpl");
        fs::write(&source, "known={=known=} unclosed={=unknown\n").expect("write template");
        let variables = BTreeMap::from([("known".to_owned(), "value".to_owned())]);
        assert_eq!(
            render_template(&source, &variables).expect("render template"),
            "known=value unclosed={=unknown\n"
        );

        fs::write(&source, "missing={=missing=}").expect("write unknown template");
        let error = render_template(&source, &variables).expect_err("unknown variable");
        assert!(matches!(
            error,
            PackageError::UnknownTemplateVariable { variable, .. } if variable == "missing"
        ));
    }

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new(name: &str) -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "dtm-package-{name}-{}-{timestamp}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temporary directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
