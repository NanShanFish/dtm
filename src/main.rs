mod config;
mod package;

use config::Config;
use package::{ApplyMode, EntryKind, PackagePlan};
use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};

const USAGE: &str = "Usage: dtm [--config <PATH>] [--dot-dir <PATH>] [--semi-force | --force] [--dry-run] <PACKAGE>";

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

    let config = Config::load(cli.config.as_deref(), cli.dot_dir.as_deref())?;
    let package_name = cli.package.ok_or(CliError::MissingPackage)?;
    let dot_dir = config
        .variables
        .get("_dotfile_dir")
        .ok_or(CliError::MissingDotfileDirectory)?;
    let plan = PackagePlan::load(Path::new(dot_dir), &package_name, &config.variables)?;


    if cli.dry_run {
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

    let report = plan.apply(&config.variables, cli.mode)?;
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
    dot_dir: Option<PathBuf>,
    package: Option<String>,
    mode: ApplyMode,
    dry_run: bool,
    help: bool,
}

impl Cli {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, CliError> {
        let mut cli = Self {
            mode: ApplyMode::Normal,
            ..Self::default()
        };
        let mut args = args.into_iter();

        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--help" | "-h" => cli.help = true,
                "--semi-force" => {
                    if cli.mode != ApplyMode::Normal {
                        return Err(CliError::ConflictingModes);
                    }
                    cli.mode = ApplyMode::SemiForce;
                }
                "--force" => {
                    if cli.mode != ApplyMode::Normal {
                        return Err(CliError::ConflictingModes);
                    }
                    cli.mode = ApplyMode::Force;
                }
                "--dry-run" => cli.dry_run = true,
                "--config" => {
                    let path = args.next().ok_or(CliError::MissingConfigPath)?;
                    cli.config = Some(PathBuf::from(path));
                }
                "--dot-dir" => {
                    let path = args.next().ok_or(CliError::MissingDotDirectory)?;
                    cli.dot_dir = Some(PathBuf::from(path));
                }
                _ if argument.starts_with('-') => {
                    return Err(CliError::UnknownArgument(argument));
                }
                _ => {
                    if cli.package.is_some() {
                        return Err(CliError::UnexpectedArgument(argument));
                    }
                    cli.package = Some(argument);
                }
            }
        }

        Ok(cli)
    }
}

#[derive(Debug, PartialEq)]
enum CliError {
    MissingConfigPath,
    MissingDotDirectory,
    ConflictingModes,
    MissingPackage,
    MissingDotfileDirectory,
    UnexpectedArgument(String),
    UnknownArgument(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingConfigPath => write!(formatter, "--config requires a file path\n{USAGE}"),
            Self::MissingDotDirectory => {
                write!(formatter, "--dot-dir requires a directory path\n{USAGE}")
            }
            Self::ConflictingModes => {
                write!(formatter, "only one force mode can be selected\n{USAGE}")
            }
            Self::MissingPackage => write!(formatter, "a package name is required\n{USAGE}"),
            Self::MissingDotfileDirectory => {
                write!(formatter, "config variable '_dotfile_dir' is missing")
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
mod tests {
    use super::*;

    #[test]
    fn parses_config_path_as_a_separate_argument() {
        let cli = Cli::parse(["--config".to_owned(), "/tmp/dtm.toml".to_owned()])
            .expect("valid arguments");

        assert_eq!(
            cli,
            Cli {
                config: Some(PathBuf::from("/tmp/dtm.toml")),
                dot_dir: None,
                package: None,
                mode: ApplyMode::Normal,
                dry_run: false,
                help: false,
            }
        );
    }

    #[test]
    fn parses_dot_directory_as_a_separate_argument() {
        let cli = Cli::parse(["--dot-dir".to_owned(), "/tmp/dotfiles".to_owned()])
            .expect("valid arguments");

        assert_eq!(cli.dot_dir, Some(PathBuf::from("/tmp/dotfiles")));
    }

    #[test]
    fn parses_dot_directory_with_equals() {
        let cli = Cli::parse(["--dot-dir=/tmp/dotfiles".to_owned()]).expect("valid arguments");

        assert_eq!(cli.dot_dir, Some(PathBuf::from("/tmp/dotfiles")));
    }

    #[test]
    fn rejects_missing_dot_directory() {
        assert_eq!(
            Cli::parse(["--dot-dir".to_owned()]),
            Err(CliError::MissingDotDirectory)
        );
    }

    #[test]
    fn parses_package_name() {
        let cli = Cli::parse(["git".to_owned()]).expect("valid arguments");

        assert_eq!(cli.package, Some("git".to_owned()));
    }

    #[test]
    fn parses_package_with_options() {
        let cli = Cli::parse([
            "--config".to_owned(),
            "/tmp/config.yaml".to_owned(),
            "--dot-dir=/tmp/dotfiles".to_owned(),
            "git".to_owned(),
        ])
        .expect("valid arguments");

        assert_eq!(
            cli,
            Cli {
                config: Some(PathBuf::from("/tmp/config.yaml")),
                dot_dir: Some(PathBuf::from("/tmp/dotfiles")),
                package: Some("git".to_owned()),
                mode: ApplyMode::Normal,
                dry_run: false,
                help: false,
            }
        );
    }

    #[test]
    fn rejects_missing_package() {
        let cli = Cli::parse([]).expect("options are valid without a package");

        assert_eq!(cli.package, None);
    }

    #[test]
    fn parses_config_path_with_equals() {
        let cli = Cli::parse(["--config=/tmp/dtm.toml".to_owned()]).expect("valid arguments");

        assert_eq!(cli.config, Some(PathBuf::from("/tmp/dtm.toml")));
    }

    #[test]
    fn rejects_missing_config_path() {
        assert_eq!(
            Cli::parse(["--config".to_owned()]),
            Err(CliError::MissingConfigPath)
        );
    }
}
