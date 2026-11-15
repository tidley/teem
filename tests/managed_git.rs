use std::{
    fs,
    os::unix::fs::{MetadataExt, symlink},
    path::Path,
};
use teem::{
    checkouts,
    common::{git, git_text},
};

fn repo(path: &Path) -> String {
    fs::create_dir(path).unwrap();
    git(path, &["init", "-q"], None, None).unwrap();
    git(path, &["config", "user.name", "Fixture"], None, None).unwrap();
    git(
        path,
        &["config", "user.email", "fixture@example.invalid"],
        None,
        None,
    )
    .unwrap();
    fs::write(path.join("file"), "base\n").unwrap();
    git(path, &["add", "."], None, None).unwrap();
    git(path, &["commit", "-qm", "base"], None, None).unwrap();
    git_text(path, &["rev-parse", "HEAD"]).unwrap()
}

#[test]
fn managed_clones_reuse_local_objects_without_repacking_or_borrowing() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let base = repo(&source);
    let worker = checkouts::prepare(&source, &tmp.path().join("worker"), &base, None).unwrap();
    let object = format!(".git/objects/{}/{}", &base[..2], &base[2..]);
    assert!(
        worker.join(&object).is_file(),
        "Local objects must not be repacked for every managed clone"
    );
    let original = fs::metadata(source.join(&object)).unwrap();
    let cloned = fs::metadata(worker.join(&object)).unwrap();
    assert_eq!(
        (original.dev(), original.ino()),
        (cloned.dev(), cloned.ino())
    );
    assert!(!worker.join(".git/objects/info/alternates").exists());
    assert_eq!(
        git_text(&worker, &["rev-parse", "--is-shallow-repository"]).unwrap(),
        "false"
    );
    // Index, refs and later Git writes remain independent despite immutable object links.
    fs::write(worker.join("file"), "worker only\n").unwrap();
    git(&worker, &["add", "."], None, None).unwrap();
    assert!(
        git_text(&source, &["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read_to_string(source.join("file")).unwrap(), "base\n");
    assert_eq!(git_text(&source, &["rev-parse", "HEAD"]).unwrap(), base);
}

#[test]
fn managed_local_clone_dissociates_preexisting_borrowed_objects() {
    let tmp = tempfile::tempdir().unwrap();
    let original = tmp.path().join("original");
    let base = repo(&original);
    let borrowed = tmp.path().join("borrowed");
    git(
        &original,
        &[
            "clone",
            "--shared",
            "--",
            original.to_str().unwrap(),
            borrowed.to_str().unwrap(),
        ],
        None,
        None,
    )
    .unwrap();
    assert!(borrowed.join(".git/objects/info/alternates").is_file());
    let worker = checkouts::prepare(&borrowed, &tmp.path().join("worker"), &base, None).unwrap();
    assert!(!worker.join(".git/objects/info/alternates").exists());
    assert_eq!(git_text(&worker, &["rev-parse", "HEAD"]).unwrap(), base);
    git(&worker, &["fsck", "--full"], None, None).unwrap();
}

#[test]
fn managed_local_clone_refuses_symlinked_object_storage() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let base = repo(&source);
    let objects = tmp.path().join("objects");
    fs::rename(source.join(".git/objects"), &objects).unwrap();
    symlink(&objects, source.join(".git/objects")).unwrap();
    assert!(checkouts::prepare(&source, &tmp.path().join("worker"), &base, None).is_err());
    assert_eq!(fs::read_to_string(source.join("file")).unwrap(), "base\n");
}
