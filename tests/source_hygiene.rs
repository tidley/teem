use std::path::Path;

fn inspect(path: &Path) {
    for entry in std::fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let name = entry.file_name();
        if [".git", "target", "bin", "node_modules"]
            .iter()
            .any(|n| name == *n)
        {
            continue;
        }
        let kind = entry.file_type().unwrap();
        assert!(
            !["py", "pyc", "pyo"]
                .iter()
                .any(|ext| path.extension().is_some_and(|e| e == *ext)),
            "Interpreter source/artifact remains: {}",
            path.display()
        );
        if kind.is_dir() {
            inspect(&path);
        }
    }
}

#[test]
fn source_tree_has_no_interpreter_implementation_tests_or_transition_links() {
    inspect(Path::new(env!("CARGO_MANIFEST_DIR")));
}
