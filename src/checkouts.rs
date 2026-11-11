use crate::common::*;
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub fn source_revision(path: &Path) -> Result<String> {
    let path = path.canonicalize()?;
    ensure!(
        git_text(&path, &["rev-parse", "--show-toplevel"])? == path.to_string_lossy(),
        "Configured source must be a Git checkout root"
    );
    ensure!(
        git(
            &path,
            &["status", "--porcelain", "--untracked-files=all"],
            None,
            None
        )?
        .is_empty(),
        "Source has unfinished changes; none discarded"
    );
    let base = git_text(&path, &["rev-parse", "HEAD"])?;
    ensure!(
        !git(&path, &["ls-tree", "-rz", &base], None, None)?
            .split(|b| *b == 0)
            .any(|e| e.starts_with(b"160000 ")),
        "Submodule checkouts are not qualified"
    );
    Ok(base)
}
pub fn candidate_tree(path: &Path, base: &str) -> Result<String> {
    let path = path.canonicalize()?;
    ensure!(
        git_text(&path, &["rev-parse", "--show-toplevel"])? == path.to_string_lossy()
            && git_text(&path, &["rev-parse", "HEAD"])? == base
            && git_text(&path, &["rev-parse", "--is-shallow-repository"])? == "false"
            && git_text(
                &path,
                &[
                    "config",
                    "--bool",
                    "--default",
                    "false",
                    "--get",
                    "core.sparseCheckout"
                ]
            )? == "false",
        "Candidate root, base or completeness changed"
    );
    let dir = git_text(&path, &["rev-parse", "--absolute-git-dir"])?;
    let tmp = tempfile::tempdir_in(dir)?;
    let index = tmp.path().join("index");
    git(&path, &["read-tree", base], None, Some(&index))?;
    git(&path, &["add", "-A", "--", "."], None, Some(&index))?;
    Ok(
        String::from_utf8(git(&path, &["write-tree"], None, Some(&index))?)?
            .trim()
            .into(),
    )
}
pub fn validate(path: &Path, base: &str, tree: &str) -> Result<()> {
    ensure!(
        candidate_tree(path, base)? == tree,
        "Candidate no longer matches exact snapshot"
    );
    Ok(())
}
pub fn task_checkout(t: &Value) -> Result<bool> {
    let path = PathBuf::from(text(t, "checkout"));
    let base = text(&t["request"], "revision");
    if let Some(tree) = t["request"]["input_tree"].as_str() {
        validate(&path, &base, tree)?;
        return Ok(false);
    }
    ensure!(
        git_text(&path, &["rev-parse", "--show-toplevel"])? == path.to_string_lossy()
            && oid(&base)
            && git_text(&path, &["rev-parse", "HEAD"])? == base,
        "Checkout root or revision mismatch"
    );
    ensure!(
        git(
            &path,
            &["status", "--porcelain", "--untracked-files=all"],
            None,
            None
        )?
        .is_empty(),
        "Assign a clean checkout"
    );
    let sparse = git_text(
        &path,
        &[
            "config",
            "--bool",
            "--default",
            "false",
            "--get",
            "core.sparseCheckout",
        ],
    )? == "true";
    ensure!(
        t["role"] != "reviewer" || !sparse,
        "Review requires a complete checkout"
    );
    Ok(sparse)
}
pub fn verify(snapshot: &Value) -> Result<Vec<u8>> {
    let data = regular(Path::new(&text(snapshot, "patch")), 64 * 1024 * 1024)?;
    ensure!(
        sha(&data) == snapshot["sha256"],
        "Snapshot patch digest changed"
    );
    Ok(data)
}

// Keep native remote-based project discovery without copying an identity file,
// permission rows, or the source's configuration. Ambiguous/local remotes retain
// native fallback discovery. Never propagate credentials into managed clones.
fn project_remote(source: &Path) -> Result<Option<String>> {
    if git_text(source, &["remote"])? != "origin" {
        return Ok(None);
    }
    let keys = git_text(
        source,
        &[
            "config",
            "--local",
            "--no-includes",
            "--name-only",
            "--list",
        ],
    )?;
    for key in keys.lines() {
        let key = key.to_ascii_lowercase();
        if key == "include.path"
            || key.starts_with("includeif.")
            || key == "extensions.worktreeconfig"
            || key.starts_with("url.")
                && (key.ends_with(".insteadof") || key.ends_with(".pushinsteadof"))
        {
            return Ok(None);
        }
    }
    let Ok(bytes) = git(
        source,
        &["config", "--local", "--get-all", "remote.origin.url"],
        None,
        None,
    ) else {
        return Ok(None);
    };
    let Ok(value) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    let remote = value.strip_suffix('\n').unwrap_or(&value);
    if remote.trim() != remote || remote.chars().any(char::is_control) {
        return Ok(None);
    }
    let url = if let Some(scp) = remote.strip_prefix("git@") {
        let Some((host, path)) = scp.split_once(':') else {
            return Ok(None);
        };
        reqwest::Url::parse(&format!("ssh://git@{host}/{path}"))
    } else {
        reqwest::Url::parse(remote)
    };
    let Ok(url) = url else {
        return Ok(None);
    };
    if !matches!(url.scheme(), "https" | "ssh")
        || url.host_str().is_none()
        || url.password().is_some()
        || !(url.username().is_empty() || url.scheme() == "ssh" && url.username() == "git")
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().trim_matches('/').is_empty()
    {
        return Ok(None);
    }
    Ok(Some(remote.to_owned()))
}

pub fn prepare(
    source: &Path,
    destination: &Path,
    base: &str,
    snapshot: Option<&Value>,
) -> Result<PathBuf> {
    let source = source.canonicalize()?;
    let destination = if destination.is_absolute() {
        destination.to_path_buf()
    } else {
        std::env::current_dir()?.join(destination)
    };
    fresh_destination(&destination)?;
    let patch = snapshot.map(verify).transpose()?;
    let remote = project_remote(&source)?;
    private_dir(destination.parent().unwrap())?;
    git(
        &source,
        &[
            "clone",
            "--no-local",
            "--no-checkout",
            "--",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ],
        None,
        None,
    )?;
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o700))?;
    if let Some(remote) = remote {
        // Objects still came exclusively from source; this does not fetch/push.
        git(
            &destination,
            &["remote", "set-url", "origin", &remote],
            None,
            None,
        )?;
    }
    git(&destination, &["checkout", "--detach", base], None, None)?;
    if git(&destination, &["ls-files", "--", "AGENTS.md"], None, None)?.is_empty() {
        let mut instructions = source.join("AGENTS.md");
        if !instructions.is_file() || instructions.is_symlink() {
            instructions = home().join(".config/opencode/templates/AGENTS.md");
        }
        if instructions.is_file() && !instructions.is_symlink() {
            write_new(
                &destination.join("AGENTS.md"),
                &regular(&instructions, 1024 * 1024)?,
                0o600,
            )?;
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(destination.join(".git/info/exclude"))?;
            f.write_all(b"\n/AGENTS.md\n")?;
        }
    }
    if let Some(p) = patch.filter(|p| !p.is_empty()) {
        git(
            &destination,
            &["apply", "--binary", "--index", "-"],
            Some(&p),
            None,
        )?;
    }
    if let Some(s) = snapshot {
        validate(&destination, base, &text(s, "tree"))?;
    }
    Ok(destination)
}
pub fn capture(path: &Path, base: &str, scope: &[String], destination: &Path) -> Result<Value> {
    let tree = candidate_tree(path, base)?;
    let files = git(
        path,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            base,
            &tree,
            "--",
        ],
        None,
        None,
    )?;
    for raw in files.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let p = std::str::from_utf8(raw)?;
        relative(p)?;
        ensure!(
            in_scope(p, scope),
            "Worker changed files outside assigned scope"
        );
    }
    let patch = git(
        path,
        &[
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            base,
            &tree,
            "--",
        ],
        None,
        None,
    )?;
    fresh_destination(destination)?;
    private_dir(destination)?;
    let p = destination.join("candidate.patch");
    write_new(&p, &patch, 0o400)?;
    let snapshot = json!({"base":base,"tree":tree,"sha256":sha(&patch),"patch":p});
    write_new(
        &destination.join("manifest.json"),
        serde_json::to_string_pretty(&snapshot)?.as_bytes(),
        0o400,
    )?;
    Ok(snapshot)
}
pub fn enable_todos(path: &Path, scope: &[String]) -> Result<bool> {
    if scope.iter().any(|s| {
        ["opencode.json", "opencode.jsonc", ".opencode"].contains(&s.as_str())
            || s.starts_with(".opencode/")
    }) {
        return Ok(false);
    }
    let cfg = path.join("opencode.json");
    if cfg.is_symlink() {
        return Ok(false);
    }
    if cfg.exists() {
        return Ok(read_json(&cfg)
            .ok()
            .is_some_and(|v| v["experimental"]["todo"] == true));
    }
    if path.join("opencode.jsonc").exists() || path.join(".opencode").exists() {
        return Ok(false);
    }
    write_new(
        &cfg,
        b"{\"experimental\":{\"todo\":true},\"warming\":false}\n",
        0o400,
    )?;
    fs::OpenOptions::new()
        .append(true)
        .open(path.join(".git/info/exclude"))?
        .write_all(b"\n/opencode.json\n")?;
    if !git(
        path,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            "opencode.json",
        ],
        None,
        None,
    )?
    .is_empty()
    {
        fs::remove_file(cfg)?;
        return Ok(false);
    }
    Ok(true)
}
