use super::*;

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|argument| (*argument).to_owned()).collect()
}

#[test]
fn parses_stow_with_shared_and_stow_options() {
    let cli = Cli::parse(strings(&[
        "--config",
        "/tmp/config.yaml",
        "stow",
        "--pkgs-dir",
        "/tmp/dotfiles",
        "--dry-run",
        "-b",
        "git",
    ]))
    .expect("valid arguments");

    assert_eq!(
        cli,
        Cli {
            config: Some(PathBuf::from("/tmp/config.yaml")),
            command: Some(Command::Stow(StowCommand {
                name: "git".to_owned(),
                pkgs_dir: Some(PathBuf::from("/tmp/dotfiles")),
                mode: ApplyMode::Normal,
                backup: true,
                dry_run: true,
            })),
            help: false,
        }
    );
}

#[test]
fn parses_short_force_modes() {
    assert!(matches!(
        Cli::parse(strings(&["stow", "-s", "git"]))
            .expect("semi-force")
            .command,
        Some(Command::Stow(StowCommand {
            mode: ApplyMode::SemiForce,
            ..
        }))
    ));
    assert!(matches!(
        Cli::parse(strings(&["stow", "-f", "git"]))
            .expect("force")
            .command,
        Some(Command::Stow(StowCommand {
            mode: ApplyMode::Force,
            ..
        }))
    ));
}

#[test]
fn parses_remove_in_safe_and_skip_unmanaged_modes() {
    let safe = Cli::parse(strings(&["rm", "git"])).expect("safe remove");
    assert_eq!(
        safe.command,
        Some(Command::Remove(RemoveCommand {
            name: "git".to_owned(),
            skip_unmanaged: false,
        }))
    );

    let skip = Cli::parse(strings(&[
        "--config",
        "/tmp/config.yaml",
        "rm",
        "--skip-unmanaged",
        "git",
    ]))
    .expect("remove while skipping unmanaged targets");
    assert_eq!(skip.config, Some(PathBuf::from("/tmp/config.yaml")));
    assert_eq!(
        skip.command,
        Some(Command::Remove(RemoveCommand {
            name: "git".to_owned(),
            skip_unmanaged: true,
        }))
    );
}

#[test]
fn remove_rejects_unknown_options_and_extra_packages() {
    assert_eq!(
        Cli::parse(strings(&["rm", "--force", "git"])),
        Err(CliError::UnknownArgument("--force".to_owned()))
    );
    assert_eq!(
        Cli::parse(strings(&["rm", "git", "tmux"])),
        Err(CliError::UnexpectedArgument("tmux".to_owned()))
    );
    assert_eq!(Cli::parse(strings(&["rm"])), Err(CliError::MissingPackage));
}

#[test]
fn parses_restore_with_shared_config_option() {
    let cli = Cli::parse(strings(&["--config", "/tmp/config.yaml", "restore", "git"]))
        .expect("valid restore arguments");

    assert_eq!(cli.config, Some(PathBuf::from("/tmp/config.yaml")));
    assert_eq!(
        cli.command,
        Some(Command::Restore(RestoreCommand {
            name: "git".to_owned(),
        }))
    );
}

#[test]
fn restore_rejects_stow_options_and_extra_packages() {
    assert_eq!(
        Cli::parse(strings(&["restore", "--force", "git"])),
        Err(CliError::UnknownArgument("--force".to_owned()))
    );
    assert_eq!(
        Cli::parse(strings(&["restore", "git", "tmux"])),
        Err(CliError::UnexpectedArgument("tmux".to_owned()))
    );
    assert_eq!(
        Cli::parse(strings(&["restore"])),
        Err(CliError::MissingPackage)
    );
}

#[test]
fn rejects_bare_package_name() {
    assert_eq!(
        Cli::parse(strings(&["git"])),
        Err(CliError::UnknownCommand("git".to_owned()))
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
fn parses_backup_directory_config_commands() {
    assert_eq!(
        Cli::parse(strings(&["config", "get", "backup_dir"]))
            .expect("get backup directory")
            .command,
        Some(Command::Config(ConfigCommand::GetBackupDir))
    );
    assert_eq!(
        Cli::parse(strings(&["config", "set", "backup_dir", "./backup"]))
            .expect("set backup directory")
            .command,
        Some(Command::Config(ConfigCommand::SetBackupDir(PathBuf::from(
            "./backup"
        ))))
    );
}

#[test]
fn parses_config_list() {
    let cli = Cli::parse(strings(&["config", "list"])).expect("valid arguments");

    assert_eq!(cli.command, Some(Command::Config(ConfigCommand::List)));
}

#[test]
fn config_command_rejects_stow_only_options() {
    assert_eq!(
        Cli::parse(strings(&["config", "get", "pkgs_dir", "--force"])),
        Err(CliError::InvalidConfigCommand)
    );
    assert_eq!(
        Cli::parse(strings(&["config", "list", "-b"])),
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
        Cli::parse(strings(&["stow", "--pkgs-dir"])),
        Err(CliError::MissingPkgsDirectory)
    );
    assert_eq!(Cli::parse(Vec::new()).unwrap().command, None);
}
