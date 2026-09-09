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
fn resolves_path_and_variable_references_across_blocks() {
    let raw: RawConfig = serde_yaml::from_str(
        r#"
config:
  pkgs_dir: /repos/dotfiles
path:
  config_home: ${home}/.config
  local_bin: ${home}/.local/bin
  dtm_config: ${config_home}/dtm
  themed_config: ${config_home}/${theme}
  system_config: ${root}/etc/dtm
variables:
  theme: dark
  config_label: ${config_home}/label
unknown:
  ignored: true
"#,
    )
    .expect("valid config");

    let (paths, variables) =
        resolve_values(&raw.path, &raw.variables, &context()).expect("resolve values");

    assert_eq!(raw.config.pkgs_dir, Some(PathBuf::from("/repos/dotfiles")));
    assert_eq!(paths["home"], "/home/tester");
    assert_eq!(paths["root"], "/");
    assert_eq!(paths["config_home"], "/home/tester/.config");
    assert_eq!(paths["local_bin"], "/home/tester/.local/bin");
    assert_eq!(paths["dtm_config"], "/home/tester/.config/dtm");
    assert_eq!(paths["themed_config"], "/home/tester/.config/dark");
    assert_eq!(paths["system_config"], "/etc/dtm");
    assert_eq!(variables["theme"], "dark");
    assert_eq!(variables["config_label"], "/home/tester/.config/label");
}

#[test]
fn configured_paths_override_predefined_paths() {
    let paths = BTreeMap::from([
        ("home".to_owned(), "/custom/home".to_owned()),
        ("root".to_owned(), "/custom/root".to_owned()),
        ("config_home".to_owned(), "${home}/.config".to_owned()),
        ("system_config".to_owned(), "${root}/etc/dtm".to_owned()),
    ]);

    let (resolved, variables) =
        resolve_values(&paths, &BTreeMap::new(), &context()).expect("resolve paths");

    assert!(variables.is_empty());
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
    let paths = BTreeMap::from([("config_home".to_owned(), "${missing}/dtm".to_owned())]);
    let error =
        resolve_values(&paths, &BTreeMap::new(), &context()).expect_err("unknown reference");

    assert!(matches!(
        error,
        EvaluationError::UnknownReference { reference, .. } if reference == "missing"
    ));
}

#[test]
fn rejects_cyclic_references_across_blocks() {
    let paths = BTreeMap::from([("a".to_owned(), "${b}".to_owned())]);
    let variables = BTreeMap::from([("b".to_owned(), "${a}".to_owned())]);
    let error = resolve_values(&paths, &variables, &context()).expect_err("cycle");

    assert!(matches!(error, EvaluationError::Cycle { .. }));
}

#[test]
fn rejects_duplicate_names_and_relative_paths() {
    let paths = BTreeMap::from([("theme".to_owned(), "/theme".to_owned())]);
    let variables = BTreeMap::from([("theme".to_owned(), "dark".to_owned())]);
    assert!(matches!(
        resolve_values(&paths, &variables, &context()),
        Err(EvaluationError::DuplicateName { name }) if name == "theme"
    ));

    let paths = BTreeMap::from([("cache".to_owned(), "relative/cache".to_owned())]);
    assert!(matches!(
        resolve_values(&paths, &BTreeMap::new(), &context()),
        Err(EvaluationError::RelativePath { name, .. }) if name == "cache"
    ));
}

#[test]
fn loaded_config_separates_paths_and_template_variables() {
    let path = temporary_path("configured-dot-dir");
    fs::write(
        &path,
        "config:\n  pkgs_dir: /configured/dotfiles\npath:\n  config_home: ${home}/.config\nvariables:\n  theme: dark\n",
    )
    .expect("write fixture");

    let config = Config::load(Some(&path), None).expect("load config");

    assert_eq!(
        config.config.pkgs_dir,
        PathBuf::from("/configured/dotfiles")
    );
    assert_eq!(config.paths["home"], fixture_home());
    assert_eq!(config.paths["root"], "/");
    assert_eq!(
        config.paths["config_home"],
        format!("{}/.config", fixture_home())
    );
    assert_eq!(config.variables["theme"], "dark");
    let template_values = config.template_values();
    assert_eq!(template_values["config_home"], config.paths["config_home"]);
    assert_eq!(template_values["theme"], "dark");
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
fn configured_runtime_config_returns_only_explicit_values() {
    let path = temporary_path("configured-runtime-only");
    fs::write(
        &path,
        "path:\n  config_home: /tmp/config\nvariables:\n  theme: dark\n",
    )
    .expect("write config");

    let configured = configured_runtime_config(Some(&path)).expect("read configured runtime");
    assert_eq!(configured, ConfiguredRuntimeConfig::default());

    let loaded = Config::load(Some(&path), None).expect("load defaults");
    assert_eq!(loaded.config.pkgs_dir, env::current_dir().unwrap());
    assert_eq!(loaded.config.backup_dir, None);
    assert_eq!(loaded.paths["config_home"], "/tmp/config");
    assert_eq!(loaded.variables["theme"], "dark");

    let _ = fs::remove_file(path);
}

#[test]
fn configured_runtime_config_preserves_explicit_values_without_loading_defaults() {
    let path = temporary_path("configured-runtime-values");
    fs::write(
        &path,
        "config:\n  pkgs_dir: relative/packages\n  backup_dir: relative/backups\n",
    )
    .expect("write config");

    let configured = configured_runtime_config(Some(&path)).expect("read configured runtime");
    assert_eq!(
        configured.pkgs_dir,
        Some(PathBuf::from("relative/packages"))
    );
    assert_eq!(
        configured.backup_dir,
        Some(PathBuf::from("relative/backups"))
    );

    let error = Config::load(Some(&path), None).expect_err("relative runtime paths");
    assert!(matches!(error, ConfigError::RelativePkgsDirectory { .. }));

    let _ = fs::remove_file(path);
}

#[test]
fn both_loaders_treat_a_missing_file_as_no_explicit_configuration() {
    let path = temporary_path("missing-runtime-config");

    assert_eq!(
        configured_runtime_config(Some(&path)).expect("read missing config"),
        ConfiguredRuntimeConfig::default()
    );
    let loaded = Config::load(Some(&path), None).expect("load defaults from missing config");
    assert_eq!(loaded.config.pkgs_dir, env::current_dir().unwrap());
    assert_eq!(loaded.config.backup_dir, None);
}

#[test]
fn set_pkgs_directory_preserves_yaml_layout_and_comments() {
    let fixture = temporary_path("preserve-pkgs-dir-layout");
    fs::create_dir_all(&fixture).expect("create fixture");
    let pkgs_dir = fixture.join("packages");
    fs::create_dir_all(&pkgs_dir).expect("create packages directory");
    let path = fixture.join("config.yaml");
    let original = "# Keep this header.\npath:\n  config_home: /tmp/config\n\n# Keep path above config.\nconfig:\n  # Existing package directory.\n  old: value\n\n# Keep this trailing comment.\n";
    fs::write(&path, original).expect("write config");

    set_pkgs_dir(Some(&path), &pkgs_dir).expect("set packages directory");
    let updated = fs::read_to_string(&path).expect("read updated config");

    assert!(updated.starts_with("# Keep this header.\npath:\n"));
    assert!(updated.find("path:").unwrap() < updated.find("config:").unwrap());
    assert!(updated.contains("# Keep path above config.\n"));
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
            "config:\n  pkgs_dir: {}\npath:\n  config_home: /tmp/config\n",
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
    assert_eq!(config.paths["config_home"], "/tmp/config");
}

#[test]
fn set_backup_directory_preserves_existing_config_layout() {
    let fixture = temporary_path("set-backup-dir");
    fs::create_dir_all(&fixture).expect("create fixture");
    let backup_dir = fixture.join("backup");
    let config_path = fixture.join("config.yaml");
    let original =
        "path:\n  config_home: /tmp/config\n\nconfig:\n  pkgs_dir: /repos/dotfiles # keep\n";
    fs::write(&config_path, original).expect("write config");

    let (_, configured_backup) =
        set_backup_dir(Some(&config_path), &backup_dir).expect("set backup directory");
    let updated = fs::read_to_string(&config_path).expect("read updated config");
    let configured = configured_runtime_config(Some(&config_path)).expect("read config");

    assert_eq!(configured_backup, backup_dir.canonicalize().unwrap());
    assert_eq!(configured.backup_dir, Some(configured_backup.clone()));
    assert_eq!(configured.pkgs_dir, Some(PathBuf::from("/repos/dotfiles")));
    assert!(updated.starts_with("path:\n  config_home: /tmp/config\n\nconfig:\n"));
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
