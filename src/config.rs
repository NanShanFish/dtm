use serde::{Deserialize, Serialize};
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

#[derive(Debug)]
pub struct Config {
    pub config: RuntimeConfig,
    pub variables: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct RuntimeConfig {
    pub pkgs_dir: PathBuf,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct RawConfig {
    #[serde(default)]
    config: RawRuntimeConfig,

    #[serde(default, alias = "variable")]
    variables: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct RawRuntimeConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pkgs_dir: Option<PathBuf>,
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
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let raw: RawConfig =
            serde_yaml::from_str(&contents).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        let context = EvaluationContext::from_environment()?;
        let pkgs_dir =
            resolve_pkgs_dir(raw.config.pkgs_dir.as_deref(), &context, pkgs_dir_override)?;
        let variables = resolve_variables(&raw.variables, &context).map_err(|source| {
            ConfigError::Evaluate {
                path: path.to_path_buf(),
                source,
            }
        })?;

        Ok(Self {
            config: RuntimeConfig { pkgs_dir },
            variables,
        })
    }
}

pub fn configured_pkgs_dir(path: Option<&Path>) -> Result<Option<PathBuf>, ConfigError> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => default_config_path()?,
    };
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Read { path, source });
        }
    };
    let raw: RawConfig =
        serde_yaml::from_str(&contents).map_err(|source| ConfigError::Parse { path, source })?;
    Ok(raw.config.pkgs_dir)
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

    let original = match fs::read_to_string(&path) {
        Ok(contents) => {
            serde_yaml::from_str::<RawConfig>(&contents).map_err(|source| ConfigError::Parse {
                path: path.clone(),
                source,
            })?;
            contents
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.clone(),
                source,
            });
        }
    };
    let contents = update_pkgs_dir_yaml(&original, &pkgs_dir);
    write_config_atomically(&path, contents.as_bytes())?;

    Ok((path, pkgs_dir))
}

fn update_pkgs_dir_yaml(original: &str, pkgs_dir: &Path) -> String {
    let value = yaml_scalar(&pkgs_dir.to_string_lossy());
    let mut lines: Vec<&str> = original.split_inclusive('\n').collect();
    if original.is_empty() {
        return format!("config:\n  pkgs_dir: {value}\n");
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

        if let Some(pkgs_index) =
            (config_index + 1..section_end).find(|&index| is_config_key(lines[index], "pkgs_dir"))
        {
            lines[pkgs_index] = replace_yaml_value(lines[pkgs_index], &value);
        } else {
            let insert_at = (config_index + 1..section_end)
                .rev()
                .find(|&index| !lines[index].trim().is_empty())
                .map(|index| index + 1)
                .unwrap_or(config_index + 1);
            lines.insert(
                insert_at,
                Box::leak(format!("  pkgs_dir: {value}\n").into_boxed_str()),
            );
        }
        return lines.concat();
    }

    let mut result = original.to_owned();
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&format!("config:\n  pkgs_dir: {value}\n"));
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

fn replace_yaml_value(line: &str, value: &str) -> &'static str {
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
    Box::leak(format!("{prefix} {value}{comment}{newline}").into_boxed_str())
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

fn resolve_variables(
    configured_variables: &BTreeMap<String, String>,
    context: &EvaluationContext,
) -> Result<BTreeMap<String, String>, EvaluationError> {
    let mut variables = BTreeMap::from([
        (
            "home".to_owned(),
            context.home.to_string_lossy().into_owned(),
        ),
        (
            "root".to_owned(),
            context.root.to_string_lossy().into_owned(),
        ),
    ]);
    variables.extend(configured_variables.clone());

    VariableResolver::new(&variables).resolve_all()
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
    Serialize(serde_yaml::Error),
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
            Self::Serialize(source) => write!(formatter, "could not serialize config: {source}"),
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
            Self::ResolvePkgsDirectory { source, .. } => Some(source),
            Self::PkgsDirectoryNotDirectory { .. } => None,
            Self::CreateConfigDirectory { source, .. } => Some(source),
            Self::Serialize(source) => Some(source),
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
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn context() -> EvaluationContext {
        EvaluationContext {
            home: PathBuf::from("/home/tester"),
            root: PathBuf::from("/"),
            current_dir: PathBuf::from("/work/dotfiles"),
        }
    }

    #[test]
    fn resolves_builtins_and_variable_references() {
        let raw: RawConfig = serde_yaml::from_str(
            r#"
config:
  pkgs_dir: /repos/dotfiles
variables:
  config_home: ${home}/.config
  local_bin: ${home}/.local/bin
  dtm_config: ${config_home}/dtm
  system_config: ${root}/etc/dtm
unknown:
  ignored: true
"#,
        )
        .expect("valid config");

        let variables = resolve_variables(&raw.variables, &context()).expect("resolve variables");

        assert_eq!(raw.config.pkgs_dir, Some(PathBuf::from("/repos/dotfiles")));
        assert_eq!(variables["home"], "/home/tester");
        assert_eq!(variables["root"], "/");
        assert!(!variables.contains_key("_pkgs_dir"));
        assert_eq!(variables["config_home"], "/home/tester/.config");
        assert_eq!(variables["local_bin"], "/home/tester/.local/bin");
        assert_eq!(variables["dtm_config"], "/home/tester/.config/dtm");
        assert_eq!(variables["system_config"], "/etc/dtm");
    }

    #[test]
    fn configured_values_override_predefined_variables() {
        let variables = BTreeMap::from([
            ("home".to_owned(), "/custom/home".to_owned()),
            ("root".to_owned(), "/custom/root".to_owned()),
            ("config_home".to_owned(), "${home}/.config".to_owned()),
            ("system_config".to_owned(), "${root}/etc/dtm".to_owned()),
        ]);

        let resolved = resolve_variables(&variables, &context()).expect("resolve variables");

        assert_eq!(resolved["home"], "/custom/home");
        assert_eq!(resolved["root"], "/custom/root");
        assert_eq!(resolved["config_home"], "/custom/home/.config");
        assert_eq!(resolved["system_config"], "/custom/root/etc/dtm");
    }

    #[test]
    fn resolves_pkgs_directory_priority() {
        let context = context();

        assert_eq!(
            resolve_pkgs_dir(Some(Path::new("/configured")), &context, None)
                .expect("configured path"),
            PathBuf::from("/configured")
        );
        assert_eq!(
            resolve_pkgs_dir(
                Some(Path::new("/configured")),
                &context,
                Some(Path::new("relative-cli")),
            )
            .expect("command-line path"),
            PathBuf::from("/work/dotfiles/relative-cli")
        );
        assert_eq!(
            resolve_pkgs_dir(None, &context, None).expect("current path"),
            PathBuf::from("/work/dotfiles")
        );
    }

    #[test]
    fn rejects_relative_configured_pkgs_directory() {
        let error = resolve_pkgs_dir(Some(Path::new("relative")), &context(), None)
            .expect_err("relative configured path");

        assert!(matches!(error, ConfigError::RelativePkgsDirectory { .. }));
    }

    #[test]
    fn rejects_unknown_references() {
        let variables = BTreeMap::from([("config_home".to_owned(), "${missing}/dtm".to_owned())]);
        let error = resolve_variables(&variables, &context()).expect_err("unknown reference");

        assert!(matches!(
            error,
            EvaluationError::UnknownReference { reference, .. } if reference == "missing"
        ));
    }

    #[test]
    fn rejects_cyclic_references() {
        let variables = BTreeMap::from([
            ("a".to_owned(), "${b}".to_owned()),
            ("b".to_owned(), "${a}".to_owned()),
        ]);
        let error = resolve_variables(&variables, &context()).expect_err("cycle");

        assert!(matches!(error, EvaluationError::Cycle { .. }));
    }

    #[test]
    fn loads_configured_pkgs_directory() {
        let path = temporary_path("configured-dot-dir");
        fs::write(
            &path,
            "config:\n  pkgs_dir: /configured/dotfiles\nvariables:\n  config_home: /tmp/config\n",
        )
        .expect("write fixture");

        let config = Config::load(Some(&path), None).expect("load config");

        assert_eq!(
            config.config.pkgs_dir,
            PathBuf::from("/configured/dotfiles")
        );
        assert_eq!(config.variables["home"], fixture_home());
        assert_eq!(config.variables["root"], "/");
        assert!(!config.variables.contains_key("_pkgs_dir"));
        assert_eq!(config.variables["config_home"], "/tmp/config");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn defaults_pkgs_directory_to_current_directory() {
        let path = temporary_path("default-dot-dir");
        fs::write(&path, "variables: {}\n").expect("write fixture");

        let config = Config::load(Some(&path), None).expect("load config");

        assert_eq!(config.config.pkgs_dir, env::current_dir().unwrap());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn command_line_pkgs_directory_overrides_config() {
        let path = temporary_path("dot-dir-override");
        fs::write(&path, "config:\n  pkgs_dir: /configured/dotfiles\n").expect("write fixture");

        let config =
            Config::load(Some(&path), Some(Path::new("/cli/dotfiles"))).expect("load config");

        assert_eq!(config.config.pkgs_dir, PathBuf::from("/cli/dotfiles"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn configured_pkgs_directory_is_absent_when_not_in_file() {
        let path = temporary_path("list-without-pkgs-dir");
        fs::write(&path, "variables:\n  home_name: tester\n").expect("write fixture");

        assert_eq!(configured_pkgs_dir(Some(&path)).expect("read config"), None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn set_pkgs_directory_preserves_yaml_layout_and_comments() {
        let fixture = temporary_path("preserve-pkgs-dir-layout");
        fs::create_dir_all(&fixture).expect("create fixture");
        let pkgs_dir = fixture.join("packages");
        fs::create_dir_all(&pkgs_dir).expect("create packages directory");
        let path = fixture.join("config.yaml");
        let original = "# Keep this header.\nvariables:\n  config_home: /tmp/config\n\n# Keep variables above config.\nconfig:\n  # Existing package directory.\n  old: value\n\n# Keep this trailing comment.\n";
        fs::write(&path, original).expect("write config");

        set_pkgs_dir(Some(&path), &pkgs_dir).expect("set packages directory");
        let updated = fs::read_to_string(&path).expect("read updated config");

        assert!(updated.starts_with("# Keep this header.\nvariables:\n"));
        assert!(updated.find("variables:").unwrap() < updated.find("config:").unwrap());
        assert!(updated.contains("# Keep variables above config.\n"));
        assert!(updated.contains("  # Existing package directory.\n"));
        assert!(updated.contains(&format!(
            "  pkgs_dir: {}\n\n# Keep this trailing comment.\n",
            pkgs_dir.display()
        )));
        assert!(updated.ends_with("# Keep this trailing comment.\n"));

        let _ = fs::remove_dir_all(fixture);
    }

    #[test]
    fn set_pkgs_directory_creates_and_updates_config() {
        let fixture = temporary_path("set-pkgs-dir");
        fs::create_dir_all(&fixture).expect("create fixture");
        let first_pkgs_dir = fixture.join("first");
        let second_pkgs_dir = fixture.join("second");
        let config_path = fixture.join("config/dtm/config.yaml");
        fs::create_dir_all(&first_pkgs_dir).expect("create first packages directory");
        fs::create_dir_all(&second_pkgs_dir).expect("create second packages directory");

        let (written_path, written_pkgs_dir) =
            set_pkgs_dir(Some(&config_path), &first_pkgs_dir).expect("set packages directory");
        assert_eq!(written_path, config_path);
        assert_eq!(written_pkgs_dir, first_pkgs_dir.canonicalize().unwrap());

        let mut raw: RawConfig =
            serde_yaml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        raw.variables
            .insert("config_home".to_owned(), "/tmp/config".to_owned());
        fs::write(&config_path, serde_yaml::to_string(&raw).unwrap()).unwrap();

        set_pkgs_dir(Some(&config_path), &second_pkgs_dir).expect("update packages directory");
        let config = Config::load(Some(&config_path), None).expect("load updated config");
        assert_eq!(
            config.config.pkgs_dir,
            second_pkgs_dir.canonicalize().unwrap()
        );
        assert_eq!(config.variables["config_home"], "/tmp/config");

        let _ = fs::remove_dir_all(fixture);
    }

    #[test]
    fn set_pkgs_directory_rejects_missing_or_non_directory_paths() {
        let config_path = temporary_path("invalid-set-config");
        let missing = temporary_path("missing-pkgs-dir");
        let error = set_pkgs_dir(Some(&config_path), &missing).expect_err("missing path");
        assert!(matches!(error, ConfigError::ResolvePkgsDirectory { .. }));

        let file = temporary_path("pkgs-dir-file");
        fs::write(&file, "not a directory").unwrap();
        let error = set_pkgs_dir(Some(&config_path), &file).expect_err("ordinary file");
        assert!(matches!(
            error,
            ConfigError::PkgsDirectoryNotDirectory { .. }
        ));
        let _ = fs::remove_file(file);
    }

    #[test]
    fn default_path_uses_xdg_shape() {
        let path = PathBuf::from("/tmp/example-config");
        assert_eq!(
            config_path(&path),
            PathBuf::from("/tmp/example-config/dtm/config.yaml")
        );
    }

    fn fixture_home() -> String {
        env::var("HOME").expect("HOME is set for tests")
    }

    fn temporary_path(name: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        env::temp_dir().join(format!(
            "dtm-config-{name}-{}-{timestamp}.yaml",
            std::process::id()
        ))
    }
}
