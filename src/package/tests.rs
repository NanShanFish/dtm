use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn reads_regular_and_template_files_from_a_package() {
    let fixture = TemporaryDirectory::new("package-files");
    let pkgs_dir = fixture.path();
    let package = pkgs_dir.join("tmux");
    fs::create_dir_all(package.join(".dtm/hooks")).expect("create metadata");
    fs::create_dir_all(package.join("config_home/tmux")).expect("create package files");
    fs::write(package.join(".dtm/config.yaml"), "hooks: {}\n").expect("write metadata");
    fs::write(package.join(".dtm/hooks/post-install.sh"), "#!/bin/sh\n").expect("write hook");
    fs::write(
        package.join("config_home/tmux/tmux.conf"),
        "set -g status on\n",
    )
    .expect("write regular file");

    let variables = BTreeMap::from([("config_home".to_owned(), "/home/tester/.config".to_owned())]);
    let plan = PackagePlan::load(pkgs_dir, "tmux", &variables).expect("load package");

    assert_eq!(plan.entries.len(), 1);
    assert_eq!(
        plan.entries[0],
        PackageEntry {
            source: package.join("config_home/tmux/tmux.conf"),
            target: PathBuf::from("/home/tester/.config/tmux/tmux.conf"),
            kind: EntryKind::Symlink,
        }
    );
}

#[test]
fn maps_dot_prefixed_files_to_hidden_targets() {
    let fixture = TemporaryDirectory::new("hidden-file");
    let pkgs_dir = fixture.path();
    let package = pkgs_dir.join("git");
    fs::create_dir_all(package.join("home")).expect("create package files");
    fs::write(package.join("home/dot-gitconfig"), "[user]\n").expect("write file");

    let variables = BTreeMap::from([("home".to_owned(), "/home/tester".to_owned())]);
    let plan = PackagePlan::load(pkgs_dir, "git", &variables).expect("load package");

    assert_eq!(
        plan.entries,
        vec![PackageEntry {
            source: package.join("home/dot-gitconfig"),
            target: PathBuf::from("/home/tester/.gitconfig"),
            kind: EntryKind::Symlink,
        }]
    );
}

#[test]
fn reads_template_files_and_removes_the_template_marker() {
    let fixture = TemporaryDirectory::new("template-file");
    let pkgs_dir = fixture.path();
    let package = pkgs_dir.join("tmux");
    fs::create_dir_all(package.join("config_home/tmux")).expect("create package files");
    fs::write(
        package.join("config_home/tmux/tmux.tmpl.conf"),
        "set -g status {=enabled=}\n",
    )
    .expect("write template file");

    let variables = BTreeMap::from([("config_home".to_owned(), "/home/tester/.config".to_owned())]);
    let plan = PackagePlan::load(pkgs_dir, "tmux", &variables).expect("load package");

    assert_eq!(
        plan.entries,
        vec![PackageEntry {
            source: package.join("config_home/tmux/tmux.tmpl.conf"),
            target: PathBuf::from("/home/tester/.config/tmux/tmux.conf"),
            kind: EntryKind::Template,
        }]
    );
}

#[test]
fn classifies_deployment_names_when_removing_the_last_template_marker() {
    assert_eq!(
        deployment_name(std::ffi::OsStr::new("dot-gitconfig")),
        Some((EntryKind::Symlink, ".gitconfig".into()))
    );
    assert_eq!(
        deployment_name(std::ffi::OsStr::new("dot-gitconfig.tmpl")),
        Some((EntryKind::Template, ".gitconfig".into()))
    );
    assert_eq!(
        deployment_name(std::ffi::OsStr::new("tmux.conf.tmpl")),
        Some((EntryKind::Template, "tmux.conf".into()))
    );
    assert_eq!(
        deployment_name(std::ffi::OsStr::new("tmux.tmpl.conf")),
        Some((EntryKind::Template, "tmux.conf".into()))
    );
    assert_eq!(
        deployment_name(std::ffi::OsStr::new("app.tmpl.backup.tmpl")),
        Some((EntryKind::Template, "app.tmpl.backup".into()))
    );
}

#[test]
fn rejects_unknown_variable_directory() {
    let fixture = TemporaryDirectory::new("unknown-variable");
    fs::create_dir_all(fixture.path().join("pkg/missing/file")).expect("create package");

    let error =
        PackagePlan::load(fixture.path(), "pkg", &BTreeMap::new()).expect_err("unknown variable");

    assert!(
        matches!(error, PackageError::UnknownVariableDirectory { variable, .. } if variable == "missing")
    );
}

#[test]
fn rejects_files_at_package_root() {
    let fixture = TemporaryDirectory::new("root-file");
    let package = fixture.path().join("pkg");
    fs::create_dir_all(&package).expect("create package");
    fs::write(package.join("script.sh"), "#!/bin/sh\n").expect("write file");

    let error = PackagePlan::load(fixture.path(), "pkg", &BTreeMap::new()).expect_err("root file");

    assert!(matches!(error, PackageError::UnexpectedRootEntry { .. }));
}

#[test]
fn rejects_duplicate_targets_created_by_template_name() {
    let fixture = TemporaryDirectory::new("target-collision");
    let package = fixture.path().join("pkg/config/file");
    fs::create_dir_all(&package).expect("create package");
    fs::write(package.join("app.conf"), "plain\n").expect("write file");
    fs::write(package.join("app.conf.tmpl"), "template\n").expect("write file");

    let variables = BTreeMap::from([("config".to_owned(), "/tmp/config".to_owned())]);
    let error = PackagePlan::load(fixture.path(), "pkg", &variables).expect_err("target collision");

    assert!(matches!(error, PackageError::TargetCollision { .. }));
}

#[test]
fn applies_a_symlink_and_a_template() {
    let fixture = TemporaryDirectory::new("apply");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("pkg/home");
    let target_home = fixture.path().join("target");
    fs::create_dir_all(&package).expect("create package");
    fs::write(package.join("dot-config"), "linked\n").expect("write source");
    fs::write(package.join("dot-settings.conf.tmpl"), "home={=home=}\n").expect("write template");

    let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
    let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
    let report = plan
        .apply(&variables, ApplyMode::Normal)
        .expect("apply package");

    assert_eq!(report.applied.len(), 2);
    assert_eq!(
        fs::read_link(target_home.join(".config")).unwrap(),
        package.join("dot-config")
    );
    assert_eq!(
        fs::read_to_string(target_home.join(".settings.conf")).expect("read template"),
        format!("home={}\n", target_home.display())
    );
}

#[test]
fn normal_mode_skips_different_targets_and_semi_force_replaces_only_links() {
    let fixture = TemporaryDirectory::new("modes");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("pkg/home");
    let target_home = fixture.path().join("target");
    fs::create_dir_all(&package).expect("create package");
    fs::write(package.join("dot-config"), "source\n").expect("write source");
    fs::create_dir_all(&target_home).expect("create target");
    fs::write(target_home.join(".config"), "existing\n").expect("write target");

    let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
    let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
    let report = plan
        .apply(&variables, ApplyMode::Normal)
        .expect("normal apply");
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(
        fs::read_to_string(target_home.join(".config")).unwrap(),
        "existing\n"
    );

    let report = plan
        .apply(&variables, ApplyMode::SemiForce)
        .expect("semi-force apply");
    assert_eq!(report.applied.len(), 0);
    assert_eq!(report.skipped.len(), 1);

    fs::remove_file(target_home.join(".config")).expect("remove ordinary target");
    #[cfg(unix)]
    std::os::unix::fs::symlink(package.join("dot-config"), target_home.join(".config"))
        .expect("create existing link");
    let report = plan
        .apply(&variables, ApplyMode::SemiForce)
        .expect("semi-force link apply");
    assert_eq!(report.applied.len(), 0);
    assert_eq!(report.skipped[0].reason, "already up to date");
    assert_eq!(
        fs::read_link(target_home.join(".config")).unwrap(),
        package.join("dot-config")
    );
}

#[test]
fn force_replaces_an_existing_regular_file() {
    let fixture = TemporaryDirectory::new("force");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("pkg/home");
    let target_home = fixture.path().join("target");
    fs::create_dir_all(&package).expect("create package");
    fs::write(package.join("dot-config"), "source\n").expect("write source");
    fs::create_dir_all(&target_home).expect("create target");
    fs::write(target_home.join(".config"), "existing\n").expect("write target");

    let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
    let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
    let report = plan
        .apply(&variables, ApplyMode::Force)
        .expect("force apply");

    assert_eq!(report.applied.len(), 1);
    assert_eq!(
        fs::read_link(target_home.join(".config")).unwrap(),
        package.join("dot-config")
    );
}

#[test]
fn normal_mode_does_not_backup_a_target_it_would_skip() {
    let fixture = TemporaryDirectory::new("normal-backup");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("pkg/home");
    let target_home = fixture.path().join("target");
    let backup_dir = fixture.path().join("backup");
    fs::create_dir_all(&package).expect("create package");
    fs::create_dir_all(&target_home).expect("create target");
    fs::write(package.join("dot-config"), "source\n").expect("write source");
    fs::write(target_home.join(".config"), "existing\n").expect("write target");

    let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
    let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
    let report = plan
        .apply_with_backup(&variables, ApplyMode::Normal, Some(&backup_dir))
        .expect("normal apply");

    assert_eq!(report.applied.len(), 0);
    assert_eq!(report.backups.len(), 0);
    assert_eq!(report.skipped[0].reason, "target already exists");
    assert_eq!(
        fs::read_to_string(target_home.join(".config")).unwrap(),
        "existing\n"
    );
    assert!(!backup_dir.exists());
}

#[test]
fn semi_force_backs_up_a_conflicting_link_using_reverse_mapping() {
    let fixture = TemporaryDirectory::new("semi-force-backup");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("git/home");
    let target_home = fixture.path().join("target");
    let backup_dir = fixture.path().join("backup");
    let old_source = fixture.path().join("old-gitconfig");
    fs::create_dir_all(&package).expect("create package");
    fs::create_dir_all(&target_home).expect("create target");
    fs::write(package.join("dot-gitconfig"), "source\n").expect("write source");
    fs::write(&old_source, "old\n").expect("write old source");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&old_source, target_home.join(".gitconfig"))
        .expect("create old link");

    let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
    let plan = PackagePlan::load(&pkgs_dir, "git", &variables).expect("load package");
    let report = plan
        .apply_with_backup(&variables, ApplyMode::SemiForce, Some(&backup_dir))
        .expect("semi-force apply");
    let expected_backup = backup_dir.join("git/home/dot-gitconfig");

    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.backups.len(), 1);
    assert_eq!(report.backups[0].backup, expected_backup);
    assert_eq!(fs::read_link(&expected_backup).unwrap(), old_source);
    assert_eq!(
        fs::read_link(target_home.join(".gitconfig")).unwrap(),
        package.join("dot-gitconfig")
    );

    let restored = plan
        .restore(&variables, &backup_dir)
        .expect("restore old link");
    assert_eq!(restored.restored.len(), 1);
    assert_eq!(
        fs::read_link(target_home.join(".gitconfig")).unwrap(),
        old_source
    );
    assert!(!backup_dir.join("git").exists());
}

#[test]
fn force_uses_a_fixed_backup_path_and_refuses_to_overwrite_it() {
    let fixture = TemporaryDirectory::new("force-backup");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("pkg/home");
    let target_home = fixture.path().join("target");
    let backup_dir = fixture.path().join("backup");
    fs::create_dir_all(&package).expect("create package");
    fs::create_dir_all(&target_home).expect("create target");
    fs::write(package.join("dot-config"), "source\n").expect("write source");
    fs::write(target_home.join(".config"), "first\n").expect("write first target");

    let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
    let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
    let first = plan
        .apply_with_backup(&variables, ApplyMode::Force, Some(&backup_dir))
        .expect("first force apply");
    let backup = backup_dir.join("pkg/home/dot-config");
    assert_eq!(first.backups[0].backup, backup);
    assert_eq!(fs::read_to_string(&backup).unwrap(), "first\n");

    let unchanged = plan
        .apply_with_backup(&variables, ApplyMode::Force, Some(&backup_dir))
        .expect("idempotent force apply");
    assert_eq!(unchanged.backups.len(), 0);
    assert_eq!(unchanged.skipped[0].reason, "already up to date");

    fs::remove_file(target_home.join(".config")).expect("remove deployed link");
    fs::write(target_home.join(".config"), "second\n").expect("write second target");
    let error = plan
        .apply_with_backup(&variables, ApplyMode::Force, Some(&backup_dir))
        .expect_err("existing backup must not be overwritten");

    assert!(matches!(error, PackageError::BackupAlreadyExists { path } if path == backup));
    assert_eq!(fs::read_to_string(&backup).unwrap(), "first\n");
    assert_eq!(
        fs::read_to_string(target_home.join(".config")).unwrap(),
        "second\n"
    );
}

#[test]
fn restore_uses_the_nearest_variable_and_reverses_hidden_path_components() {
    let fixture = TemporaryDirectory::new("restore-nearest-variable");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("pkg");
    let target_home = fixture.path().join("target");
    let config_home = target_home.join(".config");
    let backup_dir = fixture.path().join("backup");
    fs::create_dir_all(package.join("home/dot-config/app")).expect("create home package");
    fs::create_dir_all(package.join("config_home/app")).expect("create config package");
    fs::create_dir_all(config_home.join("app")).expect("create targets");
    fs::write(package.join("home/dot-config/app/dot-state"), "new state\n")
        .expect("write symlink source");
    fs::write(
        package.join("config_home/app/settings.tmpl"),
        "name={=name=}\n",
    )
    .expect("write template source");
    fs::write(config_home.join("app/.state"), "old state\n").expect("write old state");
    fs::write(config_home.join("app/settings"), "old settings\n").expect("write old settings");

    let variables = BTreeMap::from([
        ("home".to_owned(), target_home.display().to_string()),
        ("config_home".to_owned(), config_home.display().to_string()),
        ("name".to_owned(), "dtm".to_owned()),
    ]);
    let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
    let applied = plan
        .apply_with_backup(&variables, ApplyMode::Force, Some(&backup_dir))
        .expect("stow with backup");

    let state_backup = backup_dir.join("pkg/config_home/app/dot-state");
    let settings_backup = backup_dir.join("pkg/config_home/app/settings");
    assert_eq!(applied.backups.len(), 2);
    assert!(state_backup.is_file());
    assert!(settings_backup.is_file());
    assert!(!backup_dir.join("pkg/home/dot-config").exists());
    assert_eq!(
        fs::read_link(config_home.join("app/.state")).unwrap(),
        package.join("home/dot-config/app/dot-state")
    );
    assert_eq!(
        fs::read_to_string(config_home.join("app/settings")).unwrap(),
        "name=dtm\n"
    );

    let restored = plan
        .restore(&variables, &backup_dir)
        .expect("restore package");
    assert_eq!(restored.restored.len(), 2);
    assert_eq!(
        fs::read_to_string(config_home.join("app/.state")).unwrap(),
        "old state\n"
    );
    assert_eq!(
        fs::read_to_string(config_home.join("app/settings")).unwrap(),
        "old settings\n"
    );
    assert!(!backup_dir.join("pkg").exists());
}

#[test]
fn restore_preflights_every_target_before_moving_any_backup() {
    let fixture = TemporaryDirectory::new("restore-preflight");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("pkg/home");
    let target_home = fixture.path().join("target");
    let backup_dir = fixture.path().join("backup");
    fs::create_dir_all(&package).expect("create package");
    fs::create_dir_all(&target_home).expect("create target");
    fs::write(package.join("one"), "new one\n").expect("write source one");
    fs::write(package.join("two"), "new two\n").expect("write source two");
    fs::write(target_home.join("one"), "old one\n").expect("write target one");
    fs::write(target_home.join("two"), "old two\n").expect("write target two");

    let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
    let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
    plan.apply_with_backup(&variables, ApplyMode::Force, Some(&backup_dir))
        .expect("stow with backup");

    fs::remove_file(target_home.join("two")).expect("remove managed target");
    fs::write(target_home.join("two"), "user changed\n").expect("replace managed target");
    let error = plan
        .restore(&variables, &backup_dir)
        .expect_err("changed target must block restore");

    assert!(matches!(
        error,
        PackageError::RestoreTargetNotManaged { path } if path == target_home.join("two")
    ));
    assert_eq!(
        fs::read_link(target_home.join("one")).unwrap(),
        package.join("one")
    );
    assert_eq!(
        fs::read_to_string(target_home.join("two")).unwrap(),
        "user changed\n"
    );
    assert_eq!(
        fs::read_to_string(backup_dir.join("pkg/home/one")).unwrap(),
        "old one\n"
    );
    assert_eq!(
        fs::read_to_string(backup_dir.join("pkg/home/two")).unwrap(),
        "old two\n"
    );
}

#[test]
fn semi_force_does_not_backup_a_regular_file() {
    let fixture = TemporaryDirectory::new("semi-force-regular-backup");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("pkg/home");
    let target_home = fixture.path().join("target");
    let backup_dir = fixture.path().join("backup");
    fs::create_dir_all(&package).expect("create package");
    fs::create_dir_all(&target_home).expect("create target");
    fs::write(package.join("dot-config"), "source\n").expect("write source");
    fs::write(target_home.join(".config"), "existing\n").expect("write target");

    let variables = BTreeMap::from([("home".to_owned(), target_home.display().to_string())]);
    let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
    let report = plan
        .apply_with_backup(&variables, ApplyMode::SemiForce, Some(&backup_dir))
        .expect("semi-force apply");

    assert_eq!(report.applied.len(), 0);
    assert_eq!(report.backups.len(), 0);
    assert_eq!(report.skipped[0].reason, "target already exists");
    assert!(!backup_dir.exists());
}

#[test]
fn template_compares_rendered_content_and_force_replaces_different_file() {
    let fixture = TemporaryDirectory::new("template-conflict");
    let pkgs_dir = fixture.path().join("dotfiles");
    let package = pkgs_dir.join("pkg/home");
    let target_home = fixture.path().join("target");
    fs::create_dir_all(&package).expect("create package");
    fs::write(package.join("settings.tmpl"), "name={=name=}\n").expect("write template");

    let variables = BTreeMap::from([
        ("home".to_owned(), target_home.display().to_string()),
        ("name".to_owned(), "dtm".to_owned()),
    ]);
    let plan = PackagePlan::load(&pkgs_dir, "pkg", &variables).expect("load package");
    plan.apply(&variables, ApplyMode::Normal)
        .expect("first apply");

    let report = plan
        .apply(&variables, ApplyMode::Normal)
        .expect("second apply");
    assert_eq!(report.applied.len(), 0);
    assert_eq!(report.skipped[0].reason, "already up to date");

    fs::write(target_home.join("settings"), "changed\n").expect("change generated file");
    let report = plan
        .apply(&variables, ApplyMode::Normal)
        .expect("normal apply");
    assert_eq!(report.skipped[0].reason, "target already exists");

    let report = plan
        .apply(&variables, ApplyMode::Force)
        .expect("force apply");
    assert_eq!(report.applied.len(), 1);
    assert_eq!(
        fs::read_to_string(target_home.join("settings")).expect("read generated file"),
        "name=dtm\n"
    );
}

#[test]
fn template_rejects_unknown_variables_and_preserves_unclosed_or_multiline_markers() {
    let fixture = TemporaryDirectory::new("template-rendering");
    let source = fixture.path().join("source.tmpl");
    fs::write(&source, "known={=known=} unclosed={=unknown\n").expect("write template");
    let variables = BTreeMap::from([("known".to_owned(), "value".to_owned())]);
    assert_eq!(
        render_template(&source, &variables).expect("render template"),
        "known=value unclosed={=unknown\n"
    );

    fs::write(&source, "missing={=missing=}").expect("write unknown template");
    let error = render_template(&source, &variables).expect_err("unknown variable");
    assert!(matches!(
        error,
        PackageError::UnknownTemplateVariable { variable, .. } if variable == "missing"
    ));
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(name: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dtm-package-{name}-{}-{timestamp}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
