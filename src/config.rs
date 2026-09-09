use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const CONFIG_DIRECTORY: &str = "dtm";
const CONFIG_FILE: &str = "config.yaml";
const ROOT_DIRECTORY: &str = "/";

type Values = BTreeMap<String, String>;
type ResolvedValues = (Values, Values);

#[derive(Debug)]
pub struct Config {
    pub config: RuntimeConfig,
    pub paths: Values,
    pub variables: Values,
}

impl Config {
    pub fn template_values(&self) -> Values {
        let mut values = self.paths.clone();
        values.extend(self.variables.clone());
        values
    }
}

#[derive(Debug)]
pub struct RuntimeConfig {
    pub pkgs_dir: PathBuf,
    pub backup_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    config: RawRuntimeConfig,

    #[serde(default)]
    path: BTreeMap<String, String>,

    #[serde(default, alias = "variable")]
    variables: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawRuntimeConfig {
    pkgs_dir: Option<PathBuf>,
    backup_dir: Option<PathBuf>,
}

impl Config {
    pub fn load(
        path: Option<&Path>,
        pkgs_dir_override: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let path = match path {
            Some(path) => path.to_path_buf(),
            None => default_config_path()?,
        };

        Self::from_path(&path, pkgs_dir_override)
    }

    pub fn from_path(path: &Path, pkgs_dir_override: Option<&Path>) -> Result<Self, ConfigError> {
        let raw = load_raw_config(path)?;
        let context = EvaluationContext::from_environment()?;
        let pkgs_dir =
            resolve_pkgs_dir(raw.config.pkgs_dir.as_deref(), &context, pkgs_dir_override)?;
        let backup_dir = resolve_backup_dir(raw.config.backup_dir.as_deref())?;
        let (paths, variables) =
            resolve_values(&raw.path, &raw.variables, &context).map_err(|source| {
                ConfigError::Evaluate {
                    path: path.to_path_buf(),
                    source,
                }
            })?;

        Ok(Self {
            config: RuntimeConfig {
                pkgs_dir,
                backup_dir,
            },
            paths,
            variables,
        })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ConfiguredRuntimeConfig {
    pub pkgs_dir: Option<PathBuf>,
    pub backup_dir: Option<PathBuf>,
}

pub fn configured_runtime_config(
    path: Option<&Path>,
) -> Result<ConfiguredRuntimeConfig, ConfigError> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => default_config_path()?,
    };
    let raw = load_raw_config(&path)?;
    Ok(ConfiguredRuntimeConfig {
        pkgs_dir: raw.config.pkgs_dir,
        backup_dir: raw.config.backup_dir,
    })
}

fn load_raw_config(path: &Path) -> Result<RawConfig, ConfigError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RawConfig::default());
        }
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    serde_yaml::from_str(&contents).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

pub fn set_pkgs_dir(
    path: Option<&Path>,
    pkgs_dir: &Path,
) -> Result<(PathBuf, PathBuf), ConfigError> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => default_config_path()?,
    };
    let pkgs_dir =
        fs::canonicalize(pkgs_dir).map_err(|source| ConfigError::ResolvePkgsDirectory {
            path: pkgs_dir.to_path_buf(),
            source,
        })?;
    if !pkgs_dir.is_dir() {
        return Err(ConfigError::PkgsDirectoryNotDirectory { path: pkgs_dir });
    }

    let original = read_config_for_update(&path)?;
    let contents = update_runtime_path_yaml(&original, "pkgs_dir", &pkgs_dir);
    write_config_atomically(&path, contents.as_bytes())?;

    Ok((path, pkgs_dir))
}

pub fn set_backup_dir(
    path: Option<&Path>,
    backup_dir: &Path,
) -> Result<(PathBuf, PathBuf), ConfigError> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => default_config_path()?,
    };
    let backup_dir = if backup_dir.is_absolute() {
        backup_dir.to_path_buf()
    } else {
        env::current_dir()
            .map_err(ConfigError::CurrentDirectory)?
            .join(backup_dir)
    };
    fs::create_dir_all(&backup_dir).map_err(|source| ConfigError::CreateBackupDirectory {
        path: backup_dir.clone(),
        source,
    })?;
    let backup_dir =
        fs::canonicalize(&backup_dir).map_err(|source| ConfigError::ResolveBackupDirectory {
            path: backup_dir,
            source,
        })?;

    let original = read_config_for_update(&path)?;
    let contents = update_runtime_path_yaml(&original, "backup_dir", &backup_dir);
    write_config_atomically(&path, contents.as_bytes())?;

    Ok((path, backup_dir))
}

fn read_config_for_update(path: &Path) -> Result<String, ConfigError> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            serde_yaml::from_str::<RawConfig>(&contents).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(contents)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn update_runtime_path_yaml(original: &str, key: &str, path: &Path) -> String {
    let value = yaml_scalar(&path.to_string_lossy());
    let mut lines: Vec<String> = original.split_inclusive('\n').map(str::to_owned).collect();
    if original.is_empty() {
        return format!("config:\n  {key}: {value}\n");
    }

    let config_index = lines
        .iter()
        .position(|line| is_top_level_key(line, "config"));
    if let Some(config_index) = config_index {
        let section_end = lines
            .iter()
            .enumerate()
            .skip(config_index + 1)
            .find(|(_, line)| is_top_level_section_boundary(line))
            .map(|(index, _)| index)
            .unwrap_or(lines.len());

        if let Some(key_index) =
            (config_index + 1..section_end).find(|&index| is_config_key(&lines[index], key))
        {
            lines[key_index] = replace_yaml_value(&lines[key_index], &value);
        } else {
            let insert_at = (config_index + 1..section_end)
                .rev()
                .find(|&index| !lines[index].trim().is_empty())
                .map(|index| index + 1)
                .unwrap_or(config_index + 1);
            lines.insert(insert_at, format!("  {key}: {value}\n"));
        }
        return lines.concat();
    }

    let mut result = original.to_owned();
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&format!("config:\n  {key}: {value}\n"));
    result
}

fn yaml_scalar(value: &str) -> String {
    serde_yaml::to_string(value)
        .expect("serializing a string cannot fail")
        .trim_end()
        .to_owned()
}

fn is_top_level_key(line: &str, key: &str) -> bool {
    let without_newline = line.trim_end_matches(['\r', '\n']);
    without_newline
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with(':'))
}

fn is_top_level_section_boundary(line: &str) -> bool {
    !line.trim().is_empty()
        && line
            .chars()
            .next()
            .is_some_and(|character| !character.is_whitespace())
}

fn is_config_key(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with(':'))
}

fn replace_yaml_value(line: &str, value: &str) -> String {
    let newline = if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    let body = line.trim_end_matches(['\r', '\n']);
    let colon = body.find(':').expect("a config key contains a colon");
    let prefix = &body[..=colon];
    let comment = body.find(" #").map(|index| &body[index..]).unwrap_or("");
    format!("{prefix} {value}{comment}{newline}")
}

fn write_config_atomically(path: &Path, contents: &[u8]) -> Result<(), ConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| ConfigError::CreateConfigDirectory {
        path: parent.to_path_buf(),
        source,
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.yaml");
    let temporary = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|source| ConfigError::WriteConfig {
                path: temporary.clone(),
                source,
            })?;
        file.write_all(contents)
            .and_then(|_| file.sync_all())
            .map_err(|source| ConfigError::WriteConfig {
                path: temporary.clone(),
                source,
            })?;
        fs::rename(&temporary, path).map_err(|source| ConfigError::ReplaceConfig {
            path: path.to_path_buf(),
            source,
        })
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    let config_home = match env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => home_directory()?.join(".config"),
    };

    Ok(config_path(&config_home))
}

fn config_path(config_home: &Path) -> PathBuf {
    config_home.join(CONFIG_DIRECTORY).join(CONFIG_FILE)
}

fn home_directory() -> Result<PathBuf, ConfigError> {
    env::var_os("HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or(ConfigError::HomeNotFound)
}

#[derive(Debug)]
struct EvaluationContext {
    home: PathBuf,
    root: PathBuf,
    current_dir: PathBuf,
}

impl EvaluationContext {
    fn from_environment() -> Result<Self, ConfigError> {
        Ok(Self {
            home: home_directory()?,
            root: PathBuf::from(ROOT_DIRECTORY),
            current_dir: env::current_dir().map_err(ConfigError::CurrentDirectory)?,
        })
    }
}

fn resolve_pkgs_dir(
    configured: Option<&Path>,
    context: &EvaluationContext,
    override_path: Option<&Path>,
) -> Result<PathBuf, ConfigError> {
    if let Some(path) = override_path {
        return Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            context.current_dir.join(path)
        });
    }

    match configured {
        Some(path) if path.is_absolute() => Ok(path.to_path_buf()),
        Some(path) => Err(ConfigError::RelativePkgsDirectory {
            path: path.to_path_buf(),
        }),
        None => Ok(context.current_dir.clone()),
    }
}

fn resolve_backup_dir(configured: Option<&Path>) -> Result<Option<PathBuf>, ConfigError> {
    match configured {
        Some(path) if path.is_absolute() => Ok(Some(path.to_path_buf())),
        Some(path) => Err(ConfigError::RelativeBackupDirectory {
            path: path.to_path_buf(),
        }),
        None => Ok(None),
    }
}

fn resolve_values(
    configured_paths: &BTreeMap<String, String>,
    configured_variables: &BTreeMap<String, String>,
    context: &EvaluationContext,
) -> Result<ResolvedValues, EvaluationError> {
    for name in configured_paths.keys() {
        if configured_variables.contains_key(name) {
            return Err(EvaluationError::DuplicateName { name: name.clone() });
        }
    }

    let mut raw_paths = BTreeMap::from([
        (
            "home".to_owned(),
            context.home.to_string_lossy().into_owned(),
        ),
        (
            "root".to_owned(),
            context.root.to_string_lossy().into_owned(),
        ),
    ]);
    raw_paths.extend(configured_paths.clone());

    for name in configured_variables.keys() {
        if raw_paths.contains_key(name) {
            return Err(EvaluationError::DuplicateName { name: name.clone() });
        }
    }

    let mut raw = raw_paths.clone();
    raw.extend(configured_variables.clone());
    let resolved = VariableResolver::new(&raw).resolve_all()?;

    let mut paths = BTreeMap::new();
    for name in raw_paths.keys() {
        let value = resolved
            .get(name)
            .expect("the resolver returns every configured value")
            .clone();
        if !Path::new(&value).is_absolute() {
            return Err(EvaluationError::RelativePath {
                name: name.clone(),
                value,
            });
        }
        paths.insert(
            name.clone(),
            resolved
                .get(name)
                .expect("the resolver returns every configured value")
                .clone(),
        );
    }

    let variables = configured_variables
        .keys()
        .map(|name| {
            (
                name.clone(),
                resolved
                    .get(name)
                    .expect("the resolver returns every configured value")
                    .clone(),
            )
        })
        .collect();

    Ok((paths, variables))
}

struct VariableResolver<'a> {
    raw: &'a BTreeMap<String, String>,
    resolved: BTreeMap<String, String>,
    resolving: Vec<String>,
}

impl<'a> VariableResolver<'a> {
    fn new(raw: &'a BTreeMap<String, String>) -> Self {
        Self {
            raw,
            resolved: BTreeMap::new(),
            resolving: Vec::new(),
        }
    }

    fn resolve_all(mut self) -> Result<BTreeMap<String, String>, EvaluationError> {
        let names = self.raw.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.resolve_variable(&name)?;
        }
        Ok(self.resolved)
    }

    fn resolve_variable(&mut self, name: &str) -> Result<String, EvaluationError> {
        if let Some(value) = self.resolved.get(name) {
            return Ok(value.clone());
        }

        if let Some(start) = self.resolving.iter().position(|item| item == name) {
            let mut variables = self.resolving[start..].to_vec();
            variables.push(name.to_owned());
            return Err(EvaluationError::Cycle { variables });
        }

        let template =
            self.raw
                .get(name)
                .cloned()
                .ok_or_else(|| EvaluationError::UnknownVariable {
                    variable: name.to_owned(),
                })?;
        self.resolving.push(name.to_owned());
        let result = self.interpolate(name, &template);
        self.resolving.pop();
        let value = result?;

        self.resolved.insert(name.to_owned(), value.clone());
        Ok(value)
    }

    fn interpolate(&mut self, variable: &str, template: &str) -> Result<String, EvaluationError> {
        let mut result = String::new();
        let mut remaining = template;

        while let Some(start) = remaining.find("${") {
            append_fragment(&mut result, &remaining[..start]);
            remaining = &remaining[start + 2..];

            let end =
                remaining
                    .find('}')
                    .ok_or_else(|| EvaluationError::UnterminatedReference {
                        variable: variable.to_owned(),
                    })?;
            let reference = &remaining[..end];
            validate_reference(variable, reference)?;
            let value = self.resolve_reference(variable, reference)?;
            append_fragment(&mut result, &value);
            remaining = &remaining[end + 1..];
        }

        append_fragment(&mut result, remaining);
        Ok(result)
    }

    fn resolve_reference(
        &mut self,
        variable: &str,
        reference: &str,
    ) -> Result<String, EvaluationError> {
        match reference {
            name if self.raw.contains_key(name) => self.resolve_variable(name),
            _ => Err(EvaluationError::UnknownReference {
                variable: variable.to_owned(),
                reference: reference.to_owned(),
            }),
        }
    }
}

fn append_fragment(result: &mut String, fragment: &str) {
    if result.ends_with('/') && fragment.starts_with('/') {
        result.push_str(&fragment[1..]);
    } else {
        result.push_str(fragment);
    }
}

fn validate_reference(variable: &str, reference: &str) -> Result<(), EvaluationError> {
    let mut characters = reference.chars();
    let Some(first) = characters.next() else {
        return Err(EvaluationError::EmptyReference {
            variable: variable.to_owned(),
        });
    };

    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(EvaluationError::InvalidReference {
            variable: variable.to_owned(),
            reference: reference.to_owned(),
        });
    }

    if characters
        .any(|character| !character.is_ascii_alphanumeric() && character != '_' && character != '-')
    {
        return Err(EvaluationError::InvalidReference {
            variable: variable.to_owned(),
            reference: reference.to_owned(),
        });
    }

    Ok(())
}

#[derive(Debug)]
pub enum ConfigError {
    HomeNotFound,
    CurrentDirectory(std::io::Error),
    RelativePkgsDirectory {
        path: PathBuf,
    },
    RelativeBackupDirectory {
        path: PathBuf,
    },
    ResolveBackupDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    CreateBackupDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    ResolvePkgsDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    PkgsDirectoryNotDirectory {
        path: PathBuf,
    },
    CreateConfigDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    WriteConfig {
        path: PathBuf,
        source: std::io::Error,
    },
    ReplaceConfig {
        path: PathBuf,
        source: std::io::Error,
    },
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    Evaluate {
        path: PathBuf,
        source: EvaluationError,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeNotFound => write!(formatter, "could not determine the home directory"),
            Self::CurrentDirectory(source) => {
                write!(
                    formatter,
                    "could not determine the current directory: {source}"
                )
            }
            Self::RelativePkgsDirectory { path } => {
                write!(
                    formatter,
                    "config.pkgs_dir must be an absolute path: {}",
                    path.display()
                )
            }
            Self::RelativeBackupDirectory { path } => write!(
                formatter,
                "config.backup_dir must be an absolute path: {}",
                path.display()
            ),
            Self::ResolveBackupDirectory { path, source } => write!(
                formatter,
                "could not resolve backup directory {}: {source}",
                path.display()
            ),
            Self::CreateBackupDirectory { path, source } => write!(
                formatter,
                "could not create backup directory {}: {source}",
                path.display()
            ),
            Self::ResolvePkgsDirectory { path, source } => write!(
                formatter,
                "could not resolve packages directory {}: {source}",
                path.display()
            ),
            Self::PkgsDirectoryNotDirectory { path } => {
                write!(
                    formatter,
                    "packages directory is not a directory: {}",
                    path.display()
                )
            }
            Self::CreateConfigDirectory { path, source } => write!(
                formatter,
                "could not create config directory {}: {source}",
                path.display()
            ),
            Self::WriteConfig { path, source } => write!(
                formatter,
                "could not write config {}: {source}",
                path.display()
            ),
            Self::ReplaceConfig { path, source } => write!(
                formatter,
                "could not replace config {}: {source}",
                path.display()
            ),
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "could not read config {}: {source}",
                    path.display()
                )
            }
            Self::Parse { path, source } => {
                write!(
                    formatter,
                    "could not parse config {}: {source}",
                    path.display()
                )
            }
            Self::Evaluate { path, source } => {
                write!(
                    formatter,
                    "could not evaluate config {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::HomeNotFound => None,
            Self::CurrentDirectory(source) => Some(source),
            Self::RelativePkgsDirectory { .. } => None,
            Self::RelativeBackupDirectory { .. } => None,
            Self::ResolveBackupDirectory { source, .. }
            | Self::CreateBackupDirectory { source, .. }
            | Self::ResolvePkgsDirectory { source, .. } => Some(source),
            Self::PkgsDirectoryNotDirectory { .. } => None,
            Self::CreateConfigDirectory { source, .. } => Some(source),
            Self::WriteConfig { source, .. } => Some(source),
            Self::ReplaceConfig { source, .. } => Some(source),
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Evaluate { source, .. } => Some(source),
        }
    }
}

#[derive(Debug)]
pub enum EvaluationError {
    DuplicateName { name: String },
    RelativePath { name: String, value: String },
    EmptyReference { variable: String },
    InvalidReference { variable: String, reference: String },
    UnterminatedReference { variable: String },
    UnknownReference { variable: String, reference: String },
    UnknownVariable { variable: String },
    Cycle { variables: Vec<String> },
}

impl fmt::Display for EvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName { name } => {
                write!(formatter, "'{name}' is defined in both path and variables")
            }
            Self::RelativePath { name, value } => write!(
                formatter,
                "path '{name}' must resolve to an absolute path, got {value}"
            ),
            Self::EmptyReference { variable } => {
                write!(
                    formatter,
                    "variable '{variable}' contains an empty reference"
                )
            }
            Self::InvalidReference {
                variable,
                reference,
            } => write!(
                formatter,
                "variable '{variable}' contains invalid reference '${{{reference}}}'"
            ),
            Self::UnterminatedReference { variable } => {
                write!(
                    formatter,
                    "variable '{variable}' contains an unterminated reference"
                )
            }
            Self::UnknownReference {
                variable,
                reference,
            } => write!(
                formatter,
                "variable '{variable}' references unknown variable '{reference}'"
            ),
            Self::UnknownVariable { variable } => {
                write!(formatter, "unknown variable '{variable}'")
            }
            Self::Cycle { variables } => write!(
                formatter,
                "cyclic variable reference: {}",
                variables.join(" -> ")
            ),
        }
    }
}

impl Error for EvaluationError {}

#[cfg(test)]
mod tests;
