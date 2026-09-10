use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("dtm-pack-{name}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn paths(home: &Path, config_home: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("home".to_owned(), home.display().to_string()),
        ("config_home".to_owned(), config_home.display().to_string()),
    ])
}

#[test]
fn maps_to_nearest_path_and_installs_after_copying() {
    let fixture = TemporaryDirectory::new("nearest");
    let home = fixture.path().join("home");
    let config_home = home.join(".config");
    let source = config_home.join("app/.state");
    let pkgs_dir = fixture.path().join("packages");
    fs::create_dir_all(source.parent().unwrap()).expect("create source parent");
    fs::write(&source, "state\n").expect("write source");

    let paths = paths(&home, &config_home);
    let plan = PackPlan::load(&pkgs_dir, "app", std::slice::from_ref(&source), &paths)
        .expect("load pack plan");
    assert_eq!(
        plan.entries[0].package_path,
        pkgs_dir.join("app/config_home/app/dot-state")
    );

    let report = plan.execute(&paths, &paths).expect("execute pack");
    assert_eq!(report.packed.len(), 1);
    assert!(plan.entries[0].package_path.is_file());
    assert_eq!(
        fs::read_link(&source).expect("read installed link"),
        plan.entries[0].package_path
    );
}

#[test]
fn recursive_input_ignores_dependencies_but_explicit_input_accepts_them() {
    let fixture = TemporaryDirectory::new("ignore");
    let home = fixture.path().join("home");
    let project = home.join("project");
    let dependency = project.join("node_modules/pkg/index.js");
    let ordinary = project.join("config.js");
    let pkgs_dir = fixture.path().join("packages");
    fs::create_dir_all(dependency.parent().unwrap()).expect("create dependency parent");
    fs::write(&dependency, "dependency\n").expect("write dependency");
    fs::write(&ordinary, "ordinary\n").expect("write ordinary");
    let paths = BTreeMap::from([("home".to_owned(), home.display().to_string())]);

    let recursive = PackPlan::load(
        &pkgs_dir,
        "recursive",
        std::slice::from_ref(&project),
        &paths,
    )
    .expect("load recursive plan");
    assert_eq!(recursive.entries.len(), 1);
    assert_eq!(recursive.entries[0].source, ordinary);

    let explicit = PackPlan::load(
        &pkgs_dir,
        "explicit",
        std::slice::from_ref(&dependency),
        &paths,
    )
    .expect("load explicit plan");
    assert_eq!(explicit.entries.len(), 1);
    assert_eq!(explicit.entries[0].source, dependency);

    let explicit_directory = PackPlan::load(
        &pkgs_dir,
        "explicit-dir",
        &[project.join("node_modules")],
        &paths,
    )
    .expect("load explicitly ignored directory");
    assert_eq!(explicit_directory.entries.len(), 1);
}

#[test]
fn recursive_input_skips_the_package_repository_but_explicit_input_rejects_it() {
    let fixture = TemporaryDirectory::new("package-input");
    let home = fixture.path().join("home");
    let source = home.join("config");
    let pkgs_dir = home.join("packages");
    fs::create_dir_all(&pkgs_dir).expect("create package repository");
    fs::write(&source, "config\n").expect("write source");
    fs::write(pkgs_dir.join("unrelated"), "package\n").expect("write package content");
    let paths = BTreeMap::from([("home".to_owned(), home.display().to_string())]);

    let recursive = PackPlan::load(&pkgs_dir, "pkg", std::slice::from_ref(&home), &paths)
        .expect("recursive parent should skip package repository");
    assert_eq!(recursive.entries.len(), 1);
    assert_eq!(recursive.entries[0].source, source);

    let error = PackPlan::load(&pkgs_dir, "pkg", std::slice::from_ref(&pkgs_dir), &paths)
        .expect_err("explicit package repository must be rejected");
    assert!(matches!(error, PackError::InputContainsPackage { .. }));
}

#[test]
fn existing_package_file_can_be_replaced_without_changing_other_files() {
    let fixture = TemporaryDirectory::new("existing");
    let home = fixture.path().join("home");
    let source = home.join(".gitconfig");
    let pkgs_dir = fixture.path().join("packages");
    let package_file = pkgs_dir.join("git/home/dot-gitconfig");
    let unrelated = pkgs_dir.join("git/home/keep");
    fs::create_dir_all(package_file.parent().unwrap()).expect("create package");
    fs::create_dir_all(&home).expect("create home");
    fs::write(&package_file, "old\n").expect("write old package file");
    fs::write(&unrelated, "keep\n").expect("write unrelated package file");
    fs::write(&source, "new\n").expect("write source");
    let paths = BTreeMap::from([("home".to_owned(), home.display().to_string())]);

    let plan = PackPlan::load(&pkgs_dir, "git", std::slice::from_ref(&source), &paths)
        .expect("load pack plan");
    assert!(plan.entries[0].conflicts);
    plan.execute(&paths, &paths).expect("execute pack");

    assert_eq!(fs::read_to_string(&package_file).unwrap(), "new\n");
    assert_eq!(fs::read_to_string(&unrelated).unwrap(), "keep\n");
    assert_eq!(fs::read_link(&source).unwrap(), package_file);
}

#[test]
fn retain_skips_declined_conflicts_and_preserves_their_sources() {
    let fixture = TemporaryDirectory::new("decline");
    let home = fixture.path().join("home");
    let first = home.join("first");
    let second = home.join("second");
    let pkgs_dir = fixture.path().join("packages");
    fs::create_dir_all(pkgs_dir.join("pkg/home")).expect("create package");
    fs::create_dir_all(&home).expect("create home");
    fs::write(pkgs_dir.join("pkg/home/first"), "old\n").expect("write conflict");
    fs::write(&first, "first\n").expect("write first");
    fs::write(&second, "second\n").expect("write second");
    let paths = BTreeMap::from([("home".to_owned(), home.display().to_string())]);

    let plan = PackPlan::load(&pkgs_dir, "pkg", &[first.clone(), second.clone()], &paths)
        .expect("load pack plan")
        .retain(|entry| !entry.conflicts);
    plan.execute(&paths, &paths).expect("execute pack");

    assert_eq!(fs::read_to_string(&first).unwrap(), "first\n");
    assert_eq!(
        fs::read_to_string(pkgs_dir.join("pkg/home/first")).unwrap(),
        "old\n"
    );
    assert_eq!(
        fs::read_link(&second).unwrap(),
        pkgs_dir.join("pkg/home/second")
    );
}

#[test]
fn install_failure_restores_sources_and_package_contents() {
    let fixture = TemporaryDirectory::new("rollback");
    let home = fixture.path().join("home");
    let source = home.join("a");
    let blocked_target = home.join("z");
    let pkgs_dir = fixture.path().join("packages");
    let package_file = pkgs_dir.join("pkg/home/a");
    fs::create_dir_all(package_file.parent().unwrap()).expect("create package");
    fs::create_dir_all(&home).expect("create home");
    fs::write(&source, "new\n").expect("write source");
    fs::write(&package_file, "old\n").expect("write old package file");
    fs::write(pkgs_dir.join("pkg/home/z"), "blocked\n").expect("write blocked package file");
    fs::create_dir(&blocked_target).expect("create blocking target directory");
    let paths = BTreeMap::from([("home".to_owned(), home.display().to_string())]);

    let plan = PackPlan::load(&pkgs_dir, "pkg", std::slice::from_ref(&source), &paths)
        .expect("load pack plan");
    let error = plan.execute(&paths, &paths).expect_err("install must fail");
    assert!(matches!(
        error,
        PackError::Package(PackageError::TargetIsDirectory { .. })
    ));
    assert_eq!(fs::read_to_string(&source).unwrap(), "new\n");
    assert_eq!(fs::read_to_string(&package_file).unwrap(), "old\n");
    assert!(blocked_target.is_dir());
}

#[test]
fn source_verification_detects_changes_after_copying() {
    let fixture = TemporaryDirectory::new("source-change");
    let source = fixture.path().join("source");
    let package_path = fixture.path().join("package");
    fs::write(&source, "before\n").expect("write source");
    fs::copy(&source, &package_path).expect("copy source");
    let entry = PackEntry {
        source: source.clone(),
        package_path,
        target: source.clone(),
        conflicts: false,
    };
    verify_sources_unchanged(std::slice::from_ref(&entry)).expect("unchanged source");

    fs::write(&source, "after\n").expect("change source");
    assert!(matches!(
        verify_sources_unchanged(&[entry]),
        Err(PackError::SourceChanged(path)) if path == source
    ));
}

#[test]
fn rejects_paths_that_package_naming_cannot_round_trip() {
    let fixture = TemporaryDirectory::new("unrepresentable");
    let home = fixture.path().join("home");
    let source = home.join("literal.tmpl.conf");
    fs::create_dir_all(&home).expect("create home");
    fs::write(&source, "contents\n").expect("write source");
    let paths = BTreeMap::from([("home".to_owned(), home.display().to_string())]);

    let error = PackPlan::load(&fixture.path().join("packages"), "pkg", &[source], &paths)
        .expect_err("template-like literal must be rejected");
    assert!(matches!(error, PackError::UnrepresentablePath(_)));
}
