use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("dtm-interactive-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn directory_selection_is_three_state_and_supports_exclusion() {
    let fixture = TemporaryDirectory::new();
    let root = fixture.0.join("root");
    fs::create_dir_all(root.join("nested")).expect("create tree");
    fs::write(root.join("one"), "one").expect("write one");
    fs::write(root.join("nested/two"), "two").expect("write two");
    let mut tree = TreeSelection::load_excluding(&root, None).expect("load tree");

    assert_eq!(tree.selection_state(0), SelectionState::None);
    tree.toggle_all();
    assert_eq!(tree.selection_state(0), SelectionState::All);

    let nested_file = tree
        .nodes
        .iter()
        .position(|node| node.path.ends_with("nested/two"))
        .expect("nested file");
    tree.toggle(nested_file);
    assert_eq!(tree.selection_state(0), SelectionState::Partial);
    assert_eq!(tree.selected_paths(), vec![root.join("one")]);
}

#[test]
fn toggling_directory_selects_all_descendant_files() {
    let fixture = TemporaryDirectory::new();
    let root = fixture.0.join("root");
    fs::create_dir_all(root.join("nested/deeper")).expect("create tree");
    fs::write(root.join("nested/one"), "one").expect("write one");
    fs::write(root.join("nested/deeper/two"), "two").expect("write two");
    fs::write(root.join("outside"), "outside").expect("write outside");
    let mut tree = TreeSelection::load_excluding(&root, None).expect("load tree");
    let nested = tree
        .nodes
        .iter()
        .position(|node| node.path.ends_with("nested"))
        .expect("nested directory");

    tree.toggle(nested);
    assert_eq!(tree.selection_state(nested), SelectionState::All);
    assert_eq!(tree.selection_state(0), SelectionState::Partial);
    assert_eq!(
        tree.selected_paths(),
        vec![root.join("nested/deeper/two"), root.join("nested/one")]
    );
}

#[test]
fn scan_excludes_package_repository_inside_selected_root() {
    let fixture = TemporaryDirectory::new();
    let root = fixture.0.join("root");
    let packages = root.join("packages");
    fs::create_dir_all(&packages).expect("create package repository");
    fs::write(packages.join("managed"), "managed").expect("write package file");
    fs::write(root.join("config"), "config").expect("write config");

    let tree = TreeSelection::load_excluding(&root, Some(&packages)).expect("load tree");
    assert!(tree.nodes.iter().any(|node| node.path.ends_with("config")));
    assert!(
        tree.nodes
            .iter()
            .all(|node| !node.path.starts_with(&packages))
    );
    assert!(matches!(
        TreeSelection::load_excluding(&packages, Some(&packages)),
        Err(InteractiveError::ExcludedRoot(_))
    ));
}

#[test]
fn truncates_lines_to_terminal_width() {
    assert_eq!(truncate_line("abcdef", 6), "abcdef");
    assert_eq!(truncate_line("abcdef", 5), "ab...");
    assert_eq!(truncate_line("abcdef", 2), "ab");
    assert_eq!(truncate_line("配置文件", 7), "配置...");
}

#[test]
fn scan_omits_default_ignored_directories() {
    let fixture = TemporaryDirectory::new();
    let root = fixture.0.join("node_modules");
    fs::create_dir_all(root.join("node_modules/pkg")).expect("create dependencies");
    fs::write(root.join("node_modules/pkg/index.js"), "ignored").expect("write ignored");
    fs::write(root.join("explicit.js"), "included").expect("write included");

    let tree =
        TreeSelection::load_excluding(&root, None).expect("explicit ignored root is accepted");
    assert!(
        tree.nodes
            .iter()
            .any(|node| node.path.ends_with("explicit.js"))
    );
    assert!(
        tree.nodes
            .iter()
            .all(|node| !node.path.ends_with("pkg/index.js"))
    );
}
