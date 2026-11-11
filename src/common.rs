use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_owned()
}
pub fn array(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}
/// Syntax only: no network lookup, ownership inference, or spending authority.
pub fn issue_reference(v: &Value) -> Result<String> {
    let error = "Supply issue as owner/repo#positive-number (for example tidley/teem#8); an authorised issue is required for new managed work";
    let value = v.as_str().context(error)?;
    let (repo, number) = value.split_once('#').context(error)?;
    let (owner, name) = repo.split_once('/').context(error)?;
    ensure!(
        !owner.is_empty()
            && owner.len() <= 39
            && !owner.starts_with('-')
            && !owner.ends_with('-')
            && !owner.contains("--")
            && owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !name.is_empty()
            && name.len() <= 100
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            && !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit()),
        "{error}"
    );
    let number = number
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .context(error)?;
    Ok(format!(
        "{}/{}#{number}",
        owner.to_ascii_lowercase(),
        name.to_ascii_lowercase()
    ))
}
/// Only structured, validated associations may affect display text.
pub fn issue_prefix(request: &Value, label: &str) -> String {
    let Ok(issue) = issue_reference(&request["issue"]) else {
        return label.to_owned();
    };
    format!("{issue} {}", without_issue_prefix(&issue, label))
}
pub fn without_issue_prefix<'a>(issue: &str, mut label: &'a str) -> &'a str {
    loop {
        let (prefix, rest) = label.split_once(' ').unwrap_or((label, ""));
        if issue_reference(&json!(prefix)).ok().as_deref() != Some(issue) {
            break;
        }
        label = rest.trim_start();
    }
    label
}
pub fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
pub fn compact(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub fn sha(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
/// Keep the historical Python digest contract, including ASCII escaping and spaces.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", canonical(&json!(k)), canonical(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical).collect::<Vec<_>>().join(", ")
        ),
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap();
            if f != 0.0 && (f.abs() < 1e-4 || f.abs() >= 1e16) {
                let scientific = format!("{f:e}");
                let (mantissa, exponent) = scientific.split_once('e').unwrap();
                let exponent: i32 = exponent.parse().unwrap();
                format!(
                    "{mantissa}e{}{:02}",
                    if exponent < 0 { '-' } else { '+' },
                    exponent.abs()
                )
            } else {
                let mut number = f.to_string();
                if !number.contains('.') {
                    number.push_str(".0");
                }
                number
            }
        }
        _ => serde_json::to_string(v)
            .unwrap()
            .chars()
            .map(|c| {
                if c < '\u{7f}' {
                    c.to_string()
                } else {
                    let mut buf = [0; 2];
                    c.encode_utf16(&mut buf)
                        .iter()
                        .map(|u| format!("\\u{u:04x}"))
                        .collect()
                }
            })
            .collect(),
    }
}
pub fn digest(v: &Value) -> String {
    sha(canonical(v).as_bytes())
}
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
pub fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(p) = p.strip_prefix("~/") {
        home().join(p)
    } else {
        p.into()
    }
}
pub fn absolute(p: &Path) -> Result<PathBuf> {
    let p = if p.is_absolute() {
        p.to_owned()
    } else {
        std::env::current_dir()?.join(p)
    };
    if p.exists() {
        return Ok(p.canonicalize()?);
    }
    let parent = p.parent().context("Path has no parent")?;
    Ok(absolute(parent)?.join(p.file_name().context("Path has no name")?))
}
pub fn regular(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure!(
        f.metadata()?.is_file() && f.metadata()?.len() <= limit,
        "Invalid or oversized regular file"
    );
    let mut data = Vec::new();
    f.take(limit + 1).read_to_end(&mut data)?;
    ensure!(data.len() as u64 <= limit, "File exceeded size limit");
    Ok(data)
}
pub fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&regular(path, 16 * 1024 * 1024)?)?)
}
pub fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "Private directory must not be a symlink"
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
pub fn write_new(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}
pub fn atomic_json(path: &Path, v: &Value) -> Result<()> {
    let mut f = tempfile::NamedTempFile::new_in(path.parent().context("Missing parent")?)?;
    f.write_all(serde_json::to_string(v)?.as_bytes())?;
    f.write_all(b"\n")?;
    f.as_file().sync_all()?;
    f.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub fn fresh_destination(path: &Path) -> Result<()> {
    for p in path.ancestors() {
        if let Ok(m) = fs::symlink_metadata(p) {
            ensure!(!m.file_type().is_symlink(), "Destination follows a symlink");
        }
    }
    ensure!(
        !path.exists(),
        "Destination already exists; never overwrite or replay it"
    );
    Ok(())
}
pub fn uuid(v: &str) -> Result<uuid::Uuid> {
    let id = uuid::Uuid::parse_str(v).context("Use a canonical UUID")?;
    ensure!(id.to_string() == v, "Use a canonical UUID");
    Ok(id)
}
pub fn oid(v: &str) -> bool {
    (v.len() == 40 || v.len() == 64)
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn relative(v: &str) -> Result<()> {
    ensure!(
        !v.is_empty()
            && !v.starts_with('/')
            && !v.split('/').any(|s| ["", ".", "..", ".git"].contains(&s))
            && !v.contains(['*', '?', '[', ']', '\\', '\n', '\r', '\0']),
        "Use explicit relative paths without traversal, Git metadata, or patterns"
    );
    Ok(())
}
pub fn scope(v: &Value) -> Result<Vec<String>> {
    let a = v.as_array().context("Scope must be an array")?;
    ensure!(
        !a.is_empty() && a.len() <= 100,
        "Use one to 100 explicit scope paths"
    );
    a.iter()
        .map(|s| {
            let s = s.as_str().context("Scope path must be text")?;
            relative(s)?;
            Ok(s.into())
        })
        .collect()
}
pub fn in_scope(p: &str, scope: &[String]) -> bool {
    scope
        .iter()
        .any(|s| p == s || p.starts_with(&format!("{s}/")))
}
pub fn model(v: &Value) -> Result<Value> {
    let m = v.as_object().context("Foreman needs an explicit model")?;
    ensure!(
        m.contains_key("providerID")
            && m.contains_key("id")
            && m.keys()
                .all(|k| ["providerID", "id", "variant"].contains(&k.as_str()))
            && m.values().all(|v| v.as_str().is_some_and(|s| !s.is_empty()
                && s.len() <= 300
                && !s.contains(['\n', '\r', '\0']))),
        "Foreman needs an explicit provider, model and optional variant"
    );
    Ok(v.clone())
}
pub fn clean_env(cmd: &mut Command) {
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(k);
        }
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
}
pub fn process_group(cmd: &mut Command) {
    // SAFETY: setsid is async-signal-safe and the child has not executed user code.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
pub fn kill_group(pid: u32) {
    // Only process groups created by process_group are passed here.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}
pub fn git(
    path: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    index: Option<&Path>,
) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "protocol.file.allow=always",
        "--no-optional-locks",
        "-C",
    ])
    .arg(path)
    .args(args)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    clean_env(&mut cmd);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    process_group(&mut cmd);
    let mut child = cmd
        .spawn()
        .context("Cannot inspect assigned Git checkout")?;
    let mut stdin = child.stdin.take().unwrap();
    let bytes = input.unwrap_or_default().to_vec();
    let writer = thread::spawn(move || stdin.write_all(&bytes));
    let stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut b = Vec::new();
        stdout
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut b)
            .map(|_| b)
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() > deadline {
            kill_group(child.id());
            child.wait()?;
            let _ = writer.join();
            let _ = reader.join();
            bail!("Managed Git timed out; preserve checkout")
        }
        thread::sleep(Duration::from_millis(10));
    };
    kill_group(child.id());
    let _ = writer.join();
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("Git reader failed"))??;
    ensure!(
        status.success() && bytes.len() <= 64 * 1024 * 1024,
        "Managed Git operation failed; preserve checkout for inspection"
    );
    Ok(bytes)
}
pub fn git_text(path: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(path, args, None, None)?)?
        .trim()
        .into())
}
pub fn open_lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "State and lock files must be regular files"
    );
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}
