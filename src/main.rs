mod config;

use config::Config;
use std::env;
use std::error::Error;
use std::path::PathBuf;

const USAGE: &str = "Usage: dtm [--config <PATH>]";

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

    let _config = Config::load(cli.config.as_deref())?;
    println!("{:#?}", _config);
    Ok(())
}

#[derive(Debug, Default, PartialEq)]
struct Cli {
    config: Option<PathBuf>,
    help: bool,
}

impl Cli {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, CliError> {
        let mut cli = Self::default();
        let mut args = args.into_iter();

        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--help" | "-h" => cli.help = true,
                "--config" => {
                    let path = args.next().ok_or(CliError::MissingConfigPath)?;
                    cli.config = Some(PathBuf::from(path));
                }
                _ if let Some(path) = argument.strip_prefix("--config=") => {
                    if path.is_empty() {
                        return Err(CliError::MissingConfigPath);
                    }
                    cli.config = Some(PathBuf::from(path));
                }
                _ => return Err(CliError::UnknownArgument(argument)),
            }
        }

        Ok(cli)
    }
}

#[derive(Debug, PartialEq)]
enum CliError {
    MissingConfigPath,
    UnknownArgument(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingConfigPath => write!(formatter, "--config requires a file path\n{USAGE}"),
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
                help: false,
            }
        );
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
