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
        resolve_pkgs_dir(Some(Path::new("/configured")), &context, None).expect("configured path"),
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
fn configured_backup_directory_must_be_absolute() {
    assert_eq!(
        resolve_backup_dir(Some(Path::new("/backup"))).expect("absolute backup path"),
        Some(PathBuf::from("/backup"))
    );
    assert_eq!(resolve_backup_dir(None).expect("missing backup path"), None);
    assert!(matches!(
        resolve_backup_dir(Some(Path::new("relative-backup"))),
        Err(ConfigError::RelativeBackupDirectory { .. })
    ));
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

    let config = Config::load(Some(&path), Some(Path::new("/cli/dotfiles"))).expect("load config");

    assert_eq!(config.config.pkgs_dir, PathBuf::from("/cli/dotfiles"));
    let _ = fs::remove_file(path);
}

#[test]
fn configured_pkgs_directory_is_absent_when_not_in_file() {
    let path = temporary_path("list-without-pkgs-dir");
    fs::write(&path, "variables:\n  home_name: tester\n").expect("write fixture");

    assert_eq!(
        configured_runtime_config(Some(&path))
            .expect("read config")
            .pkgs_dir,
        None
    );
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

    fs::write(
        &config_path,
        format!(
            "config:\n  pkgs_dir: {}\nvariables:\n  config_home: /tmp/config\n",
            first_pkgs_dir.canonicalize().unwrap().display()
        ),
    )
    .unwrap();

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
fn set_backup_directory_preserves_existing_config_layout() {
    let fixture = temporary_path("set-backup-dir");
    fs::create_dir_all(&fixture).expect("create fixture");
    let backup_dir = fixture.join("backup");
    let config_path = fixture.join("config.yaml");
    let original =
        "variables:\n  config_home: /tmp/config\n\nconfig:\n  pkgs_dir: /repos/dotfiles # keep\n";
    fs::write(&config_path, original).expect("write config");

    let (_, configured_backup) =
        set_backup_dir(Some(&config_path), &backup_dir).expect("set backup directory");
    let updated = fs::read_to_string(&config_path).expect("read updated config");
    let configured = configured_runtime_config(Some(&config_path)).expect("read config");

    assert_eq!(configured_backup, backup_dir.canonicalize().unwrap());
    assert_eq!(configured.backup_dir, Some(configured_backup.clone()));
    assert_eq!(configured.pkgs_dir, Some(PathBuf::from("/repos/dotfiles")));
    assert!(updated.starts_with("variables:\n  config_home: /tmp/config\n\nconfig:\n"));
    assert!(updated.contains("  pkgs_dir: /repos/dotfiles # keep\n"));
    assert!(updated.contains(&format!("  backup_dir: {}\n", configured_backup.display())));

    let config = Config::load(Some(&config_path), None).expect("load config");
    assert_eq!(config.config.backup_dir, Some(configured_backup));
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
