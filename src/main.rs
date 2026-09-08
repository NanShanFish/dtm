mod config;
mod package;

use config::{Config, configured_runtime_config, set_backup_dir, set_pkgs_dir};
use package::{ApplyMode, EntryKind, PackagePlan};
use std::env;
use std::error::Error;
use std::path::PathBuf;

const USAGE: &str = "Usage:\n  dtm [--config <PATH>] stow [--pkgs-dir <PATH>] [-s | --semi-force | -f | --force] [-b | --backup] [--dry-run] <PACKAGE>\n  dtm [--config <PATH>] restore <PACKAGE>\n  dtm [--config <PATH>] config set pkgs_dir <PATH>\n  dtm [--config <PATH>] config set backup_dir <PATH>\n  dtm [--config <PATH>] config get pkgs_dir\n  dtm [--config <PATH>] config get backup_dir\n  dtm [--config <PATH>] config list";

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
        Command::Config(ConfigCommand::SetBackupDir(path)) => {
            let (config_path, backup_dir) = set_backup_dir(cli.config.as_deref(), &path)?;
            println!(
                "set config.backup_dir to {} in {}",
                backup_dir.display(),
                config_path.display()
            );
        }
        Command::Config(ConfigCommand::GetPkgsDir) => {
            let config = Config::load(cli.config.as_deref(), None)?;
            println!("{}", config.config.pkgs_dir.display());
        }
        Command::Config(ConfigCommand::GetBackupDir) => {
            let config = Config::load(cli.config.as_deref(), None)?;
            let backup_dir = config
                .config
                .backup_dir
                .ok_or(CliError::BackupDirectoryNotConfigured)?;
            println!("{}", backup_dir.display());
        }
        Command::Config(ConfigCommand::List) => {
            let configured = configured_runtime_config(cli.config.as_deref())?;
            if let Some(pkgs_dir) = configured.pkgs_dir {
                println!("pkgs_dir\t{}", pkgs_dir.display());
            }
            if let Some(backup_dir) = configured.backup_dir {
                println!("backup_dir\t{}", backup_dir.display());
            }
        }
        Command::Stow(command) => run_stow(cli.config.as_deref(), command)?,
        Command::Restore(command) => run_restore(cli.config.as_deref(), command)?,
    }

    Ok(())
}

fn run_stow(
    config_path: Option<&std::path::Path>,
    command: StowCommand,
) -> Result<(), Box<dyn Error>> {
    let config = Config::load(config_path, command.pkgs_dir.as_deref())?;
    let plan = PackagePlan::load(&config.config.pkgs_dir, &command.name, &config.variables)?;
    let backup_dir = if command.backup && command.mode != ApplyMode::Normal {
        Some(
            config
                .config
                .backup_dir
                .as_deref()
                .ok_or(CliError::BackupDirectoryNotConfigured)?,
        )
    } else {
        None
    };

    if command.dry_run {
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

    let report = match backup_dir {
        Some(backup_dir) => {
            plan.apply_with_backup(&config.variables, command.mode, Some(backup_dir))?
        }
        None => plan.apply(&config.variables, command.mode)?,
    };
    for backup in report.backups {
        eprintln!(
            "backed up {} -> {}",
            backup.original.display(),
            backup.backup.display()
        );
    }
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

fn run_restore(
    config_path: Option<&std::path::Path>,
    command: RestoreCommand,
) -> Result<(), Box<dyn Error>> {
    let config = Config::load(config_path, None)?;
    let backup_dir = config
        .config
        .backup_dir
        .as_deref()
        .ok_or(CliError::BackupDirectoryNotConfigured)?;
    let plan = PackagePlan::load(&config.config.pkgs_dir, &command.name, &config.variables)?;
    let report = plan.restore(&config.variables, backup_dir)?;
    for restored in report.restored {
        println!(
            "restored {} -> {}",
            restored.backup.display(),
            restored.target.display()
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
    Stow(StowCommand),
    Restore(RestoreCommand),
    Config(ConfigCommand),
}

#[derive(Debug, PartialEq)]
struct StowCommand {
    name: String,
    pkgs_dir: Option<PathBuf>,
    mode: ApplyMode,
    backup: bool,
    dry_run: bool,
}

#[derive(Debug, PartialEq)]
struct RestoreCommand {
    name: String,
}

#[derive(Debug, PartialEq)]
enum ConfigCommand {
    SetPkgsDir(PathBuf),
    SetBackupDir(PathBuf),
    GetPkgsDir,
    GetBackupDir,
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

        let command = match command_args.first().map(String::as_str) {
            Some("config") => Some(Command::Config(parse_config_command(&command_args[1..])?)),
            Some("stow") => Some(Command::Stow(parse_stow_command(&command_args[1..])?)),
            Some("restore") => Some(Command::Restore(parse_restore_command(&command_args[1..])?)),
            Some(command) => return Err(CliError::UnknownCommand(command.to_owned())),
            None => None,
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
        [action, key] if action == "get" && key == "backup_dir" => Ok(ConfigCommand::GetBackupDir),
        [action, key, value] if action == "set" && key == "pkgs_dir" => {
            Ok(ConfigCommand::SetPkgsDir(PathBuf::from(value)))
        }
        [action, key, value] if action == "set" && key == "backup_dir" => {
            Ok(ConfigCommand::SetBackupDir(PathBuf::from(value)))
        }
        _ => Err(CliError::InvalidConfigCommand),
    }
}

fn parse_restore_command(args: &[String]) -> Result<RestoreCommand, CliError> {
    let Some(name) = args.first() else {
        return Err(CliError::MissingPackage);
    };
    if name.starts_with('-') {
        return Err(CliError::UnknownArgument(name.to_owned()));
    }
    if let Some(argument) = args.get(1) {
        return Err(CliError::UnexpectedArgument(argument.to_owned()));
    }
    Ok(RestoreCommand {
        name: name.to_owned(),
    })
}

fn parse_stow_command(args: &[String]) -> Result<StowCommand, CliError> {
    let mut name = None;
    let mut pkgs_dir = None;
    let mut mode = ApplyMode::Normal;
    let mut backup = false;
    let mut dry_run = false;
    let mut args = args.iter();

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--semi-force" | "-s" => {
                if mode != ApplyMode::Normal {
                    return Err(CliError::ConflictingModes);
                }
                mode = ApplyMode::SemiForce;
            }
            "--force" | "-f" => {
                if mode != ApplyMode::Normal {
                    return Err(CliError::ConflictingModes);
                }
                mode = ApplyMode::Force;
            }
            "--backup" | "-b" => backup = true,
            "--dry-run" => dry_run = true,
            "--pkgs-dir" => {
                let path = args.next().ok_or(CliError::MissingPkgsDirectory)?;
                pkgs_dir = Some(PathBuf::from(path));
            }
            _ if argument.starts_with('-') => {
                return Err(CliError::UnknownArgument(argument.to_owned()));
            }
            _ => {
                if name.is_some() {
                    return Err(CliError::UnexpectedArgument(argument.to_owned()));
                }
                name = Some(argument.to_owned());
            }
        }
    }

    Ok(StowCommand {
        name: name.ok_or(CliError::MissingPackage)?,
        pkgs_dir,
        mode,
        backup,
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
    BackupDirectoryNotConfigured,
    InvalidConfigCommand,
    UnknownCommand(String),
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
            Self::MissingCommand => write!(formatter, "a command is required\n{USAGE}"),
            Self::MissingPackage => write!(formatter, "a package name is required\n{USAGE}"),
            Self::BackupDirectoryNotConfigured => {
                write!(formatter, "config.backup_dir is not configured")
            }
            Self::InvalidConfigCommand => write!(formatter, "invalid config command\n{USAGE}"),
            Self::UnknownCommand(command) => {
                write!(formatter, "unknown command '{command}'\n{USAGE}")
            }
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
#[path = "main/tests.rs"]
mod tests;
