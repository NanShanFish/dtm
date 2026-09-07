mod config;
mod package;

use config::{Config, configured_pkgs_dir, set_pkgs_dir};
use package::{ApplyMode, EntryKind, PackagePlan};
use std::env;
use std::error::Error;
use std::path::PathBuf;

const USAGE: &str = "Usage:\n  dtm [--config <PATH>] [--pkgs-dir <PATH>] [--semi-force | --force] [--dry-run] <PACKAGE>\n  dtm [--config <PATH>] config set pkgs_dir <PATH>\n  dtm [--config <PATH>] config get pkgs_dir\n  dtm [--config <PATH>] config list";

fn main() {
    if let Err(error) = run() {
        eprintln!("dtm: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse(env::args().skip(1))?;

    if cli.help {
        println!("{USAGE}");
        return Ok(());
    }

    match cli.command.ok_or(CliError::MissingCommand)? {
        Command::Config(ConfigCommand::SetPkgsDir(path)) => {
            let (config_path, pkgs_dir) = set_pkgs_dir(cli.config.as_deref(), &path)?;
            println!(
                "set config.pkgs_dir to {} in {}",
                pkgs_dir.display(),
                config_path.display()
            );
        }
        Command::Config(ConfigCommand::GetPkgsDir) => {
            let config = Config::load(cli.config.as_deref(), None)?;
            println!("{}", config.config.pkgs_dir.display());
        }
        Command::Config(ConfigCommand::List) => {
            if let Some(pkgs_dir) = configured_pkgs_dir(cli.config.as_deref())? {
                println!("pkgs_dir\t{}", pkgs_dir.display());
            }
        }
        Command::Package(package) => run_package(cli.config.as_deref(), package)?,
    }

    Ok(())
}

fn run_package(
    config_path: Option<&std::path::Path>,
    package: PackageCommand,
) -> Result<(), Box<dyn Error>> {
    let config = Config::load(config_path, package.pkgs_dir.as_deref())?;
    let plan = PackagePlan::load(&config.config.pkgs_dir, &package.name, &config.variables)?;

    if package.dry_run {
        for entry in &plan.entries {
            println!(
                "{}\t{}\t{}",
                entry.source.display(),
                entry.target.display(),
                entry_kind_name(entry.kind)
            );
        }
        return Ok(());
    }

    let report = plan.apply(&config.variables, package.mode)?;
    for skipped in report.skipped {
        eprintln!(
            "warning: skipped {} -> {}: {}",
            skipped.entry.source.display(),
            skipped.entry.target.display(),
            skipped.reason
        );
    }
    Ok(())
}

fn entry_kind_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Symlink => "symlink",
        EntryKind::Template => "template",
    }
}

#[derive(Debug, Default, PartialEq)]
struct Cli {
    config: Option<PathBuf>,
    command: Option<Command>,
    help: bool,
}

#[derive(Debug, PartialEq)]
enum Command {
    Package(PackageCommand),
    Config(ConfigCommand),
}

#[derive(Debug, PartialEq)]
struct PackageCommand {
    name: String,
    pkgs_dir: Option<PathBuf>,
    mode: ApplyMode,
    dry_run: bool,
}

#[derive(Debug, PartialEq)]
enum ConfigCommand {
    SetPkgsDir(PathBuf),
    GetPkgsDir,
    List,
}

impl Cli {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, CliError> {
        let mut config = None;
        let mut help = false;
        let mut command_args = Vec::new();
        let mut args = args.into_iter();

        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--config" => {
                    let path = args.next().ok_or(CliError::MissingConfigPath)?;
                    config = Some(PathBuf::from(path));
                }
                "--help" | "-h" => help = true,
                _ => command_args.push(argument),
            }
        }

        let command = if command_args.first().map(String::as_str) == Some("config") {
            Some(Command::Config(parse_config_command(&command_args[1..])?))
        } else if command_args.is_empty() {
            None
        } else {
            Some(Command::Package(parse_package_command(command_args)?))
        };

        Ok(Self {
            config,
            command,
            help,
        })
    }
}

fn parse_config_command(args: &[String]) -> Result<ConfigCommand, CliError> {
    match args {
        [action] if action == "list" => Ok(ConfigCommand::List),
        [action, key] if action == "get" && key == "pkgs_dir" => Ok(ConfigCommand::GetPkgsDir),
        [action, key, value] if action == "set" && key == "pkgs_dir" => {
            Ok(ConfigCommand::SetPkgsDir(PathBuf::from(value)))
        }
        _ => Err(CliError::InvalidConfigCommand),
    }
}

fn parse_package_command(args: Vec<String>) -> Result<PackageCommand, CliError> {
    let mut name = None;
    let mut pkgs_dir = None;
    let mut mode = ApplyMode::Normal;
    let mut dry_run = false;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--semi-force" => {
                if mode != ApplyMode::Normal {
                    return Err(CliError::ConflictingModes);
                }
                mode = ApplyMode::SemiForce;
            }
            "--force" => {
                if mode != ApplyMode::Normal {
                    return Err(CliError::ConflictingModes);
                }
                mode = ApplyMode::Force;
            }
            "--dry-run" => dry_run = true,
            "--pkgs-dir" => {
                let path = args.next().ok_or(CliError::MissingPkgsDirectory)?;
                pkgs_dir = Some(PathBuf::from(path));
            }
            _ if argument.starts_with('-') => return Err(CliError::UnknownArgument(argument)),
            _ => {
                if name.is_some() {
                    return Err(CliError::UnexpectedArgument(argument));
                }
                name = Some(argument);
            }
        }
    }

    Ok(PackageCommand {
        name: name.ok_or(CliError::MissingPackage)?,
        pkgs_dir,
        mode,
        dry_run,
    })
}

#[derive(Debug, PartialEq)]
enum CliError {
    MissingConfigPath,
    MissingPkgsDirectory,
    ConflictingModes,
    MissingCommand,
    MissingPackage,
    InvalidConfigCommand,
    UnexpectedArgument(String),
    UnknownArgument(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingConfigPath => write!(formatter, "--config requires a file path\n{USAGE}"),
            Self::MissingPkgsDirectory => {
                write!(formatter, "--pkgs-dir requires a directory path\n{USAGE}")
            }
            Self::ConflictingModes => {
                write!(formatter, "only one force mode can be selected\n{USAGE}")
            }
            Self::MissingCommand => write!(formatter, "a command or package is required\n{USAGE}"),
            Self::MissingPackage => write!(formatter, "a package name is required\n{USAGE}"),
            Self::InvalidConfigCommand => write!(formatter, "invalid config command\n{USAGE}"),
            Self::UnexpectedArgument(argument) => {
                write!(formatter, "unexpected argument '{argument}'\n{USAGE}")
            }
            Self::UnknownArgument(argument) => {
                write!(formatter, "unknown argument '{argument}'\n{USAGE}")
            }
        }
    }
}

impl Error for CliError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|argument| (*argument).to_owned()).collect()
    }

    #[test]
    fn parses_package_with_shared_and_package_options() {
        let cli = Cli::parse(strings(&[
            "--config",
            "/tmp/config.yaml",
            "--pkgs-dir",
            "/tmp/dotfiles",
            "--dry-run",
            "git",
        ]))
        .expect("valid arguments");

        assert_eq!(
            cli,
            Cli {
                config: Some(PathBuf::from("/tmp/config.yaml")),
                command: Some(Command::Package(PackageCommand {
                    name: "git".to_owned(),
                    pkgs_dir: Some(PathBuf::from("/tmp/dotfiles")),
                    mode: ApplyMode::Normal,
                    dry_run: true,
                })),
                help: false,
            }
        );
    }

    #[test]
    fn parses_config_set_with_shared_config_option() {
        let cli = Cli::parse(strings(&[
            "--config",
            "/tmp/config.yaml",
            "config",
            "set",
            "pkgs_dir",
            "./dotfiles",
        ]))
        .expect("valid arguments");

        assert_eq!(cli.config, Some(PathBuf::from("/tmp/config.yaml")));
        assert_eq!(
            cli.command,
            Some(Command::Config(ConfigCommand::SetPkgsDir(PathBuf::from(
                "./dotfiles"
            ))))
        );
    }

    #[test]
    fn parses_config_get() {
        let cli = Cli::parse(strings(&["config", "get", "pkgs_dir"])).expect("valid arguments");

        assert_eq!(
            cli.command,
            Some(Command::Config(ConfigCommand::GetPkgsDir))
        );
    }

    #[test]
    fn parses_config_list() {
        let cli = Cli::parse(strings(&["config", "list"])).expect("valid arguments");

        assert_eq!(cli.command, Some(Command::Config(ConfigCommand::List)));
    }

    #[test]
    fn config_command_rejects_package_only_options() {
        assert_eq!(
            Cli::parse(strings(&["config", "get", "pkgs_dir", "--force"])),
            Err(CliError::InvalidConfigCommand)
        );
        assert_eq!(
            Cli::parse(strings(&[
                "config",
                "get",
                "pkgs_dir",
                "--pkgs-dir",
                "/tmp"
            ])),
            Err(CliError::InvalidConfigCommand)
        );
    }

    #[test]
    fn rejects_missing_option_values_and_package() {
        assert_eq!(
            Cli::parse(strings(&["--config"])),
            Err(CliError::MissingConfigPath)
        );
        assert_eq!(
            Cli::parse(strings(&["--pkgs-dir"])),
            Err(CliError::MissingPkgsDirectory)
        );
        assert_eq!(Cli::parse(Vec::new()).unwrap().command, None);
    }
}
