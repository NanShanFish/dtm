use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_DIRECTORY: &str = "dtm";
const CONFIG_FILE: &str = "config.yaml";
const ROOT_DIRECTORY: &str = "/";

#[derive(Debug)]
pub struct Config {
    pub variables: BTreeMap<String, String>,
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default, alias = "variable")]
    variables: BTreeMap<String, String>,

    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml::Value>,
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let path = match path {
            Some(path) => path.to_path_buf(),
            None => default_config_path()?,
        };

        Self::from_path(&path)
    }

    pub fn from_path(path: &Path) -> Result<Self, ConfigError> {
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
        let variables = resolve_variables(&raw.variables, &context).map_err(|source| {
            ConfigError::Evaluate {
                path: path.to_path_buf(),
                source,
            }
        })?;

        Ok(Self {
            variables,
            extra: raw.extra,
        })
    }
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
}

impl EvaluationContext {
    fn from_environment() -> Result<Self, ConfigError> {
        Ok(Self {
            home: home_directory()?,
            root: PathBuf::from(ROOT_DIRECTORY),
        })
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
            Self::Cycle { variables } => {
                write!(
                    formatter,
                    "cyclic variable reference: {}",
                    variables.join(" -> ")
                )
            }
        }
    }
}

impl Error for EvaluationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn resolves_builtins_and_variable_references() {
        let raw: RawConfig = serde_yaml::from_str(
            r#"
variables:
  config_home: ${home}/.config
  local_bin: ${home}/.local/bin
  dtm_config: ${config_home}/dtm
  system_config: ${root}/etc/dtm
profile:
  enabled: true
"#,
        )
        .expect("valid config");
        let context = EvaluationContext {
            home: PathBuf::from("/home/tester"),
            root: PathBuf::from("/"),
        };

        let variables = resolve_variables(&raw.variables, &context).expect("resolve variables");

        assert_eq!(variables["home"], "/home/tester");
        assert_eq!(variables["root"], "/");
        assert_eq!(variables["config_home"], "/home/tester/.config");
        assert_eq!(variables["local_bin"], "/home/tester/.local/bin");
        assert_eq!(variables["dtm_config"], "/home/tester/.config/dtm");
        assert_eq!(variables["system_config"], "/etc/dtm");
        assert_eq!(raw.extra["profile"]["enabled"].as_bool(), Some(true));
    }

    #[test]
    fn configured_values_override_predefined_variables() {
        let variables = BTreeMap::from([
            ("home".to_owned(), "/custom/home".to_owned()),
            ("root".to_owned(), "/custom/root".to_owned()),
            ("config".to_owned(), "${home}/.config".to_owned()),
            ("system_config".to_owned(), "${root}/etc/dtm".to_owned()),
        ]);
        let context = EvaluationContext {
            home: PathBuf::from("/home/tester"),
            root: PathBuf::from("/"),
        };

        let resolved = resolve_variables(&variables, &context).expect("resolve variables");

        assert_eq!(resolved["home"], "/custom/home");
        assert_eq!(resolved["root"], "/custom/root");
        assert_eq!(resolved["config"], "/custom/home/.config");
        assert_eq!(resolved["system_config"], "/custom/root/etc/dtm");
    }

    #[test]
    fn rejects_unknown_references() {
        let variables = BTreeMap::from([("config".to_owned(), "${MISSING}/dtm".to_owned())]);
        let context = EvaluationContext {
            home: PathBuf::from("/home/tester"),
            root: PathBuf::from("/"),
        };

        let error = resolve_variables(&variables, &context).expect_err("unknown reference");

        assert!(matches!(
            error,
            EvaluationError::UnknownReference { reference, .. } if reference == "MISSING"
        ));
    }

    #[test]
    fn rejects_cyclic_references() {
        let variables = BTreeMap::from([
            ("a".to_owned(), "${b}".to_owned()),
            ("b".to_owned(), "${a}".to_owned()),
        ]);
        let context = EvaluationContext {
            home: PathBuf::from("/home/tester"),
            root: PathBuf::from("/"),
        };

        let error = resolve_variables(&variables, &context).expect_err("cycle");

        assert!(matches!(error, EvaluationError::Cycle { .. }));
    }

    #[test]
    fn loads_an_explicit_yaml_file() {
        let path = temporary_path("explicit");
        fs::write(&path, "variables:\n  config_home: /tmp/config\n").expect("write fixture");

        let config = Config::load(Some(&path)).expect("load config");

        assert_eq!(config.variables["home"], fixture_home());
        assert_eq!(config.variables["root"], "/");
        assert_eq!(config.variables["config_home"], "/tmp/config");
        let _ = fs::remove_file(path);
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
