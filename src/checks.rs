use crate::common::*;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub fn normalise(value: &Value) -> Result<Value> {
    let a = value.as_array().context("Approve one to eight checks")?;
    ensure!(!a.is_empty() && a.len() <= 8, "Approve one to eight checks");
    let mut names = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for raw in a {
        let m = raw.as_object().context("Invalid check policy")?;
        ensure!(
            m.keys().all(
                |k| ["name", "kind", "argv", "cwd", "timeout", "output_limit"]
                    .contains(&k.as_str())
            ),
            "Unknown check policy field"
        );
        let mut c = json!({"cwd":".","timeout":300,"output_limit":1048576});
        for (k, v) in m {
            c[k] = v.clone();
        }
        let name = text(&c, "name");
        let argv = c["argv"].as_array().context("Exact argv required")?;
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                && names.insert(name)
                && ["test", "build", "inspect"].contains(&text(&c, "kind").as_str())
                && !argv.is_empty()
                && argv.len() <= 64
                && argv.iter().all(|v| v
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s.len() <= 4096 && !s.contains('\0'))),
            "Invalid check name, kind or argv"
        );
        let program = argv[0].as_str().unwrap();
        ensure!(
            !program.contains('/')
                && ![
                    "sh", "bash", "zsh", "fish", "env", "sudo", "rm", "ssh", "scp", "curl", "wget"
                ]
                .contains(&program)
                && (program != "git"
                    || argv.get(1).and_then(Value::as_str).is_some_and(|a| [
                        "status",
                        "diff",
                        "show",
                        "log",
                        "rev-parse",
                        "ls-files"
                    ]
                    .contains(&a))),
            "Shell launchers or mutation commands are not approved checks"
        );
        ensure!(
            c["timeout"]
                .as_f64()
                .is_some_and(|n| n > 0.0 && n <= 1800.0)
                && c["output_limit"]
                    .as_u64()
                    .is_some_and(|n| (1024..=1048576).contains(&n)),
            "Check timeout/output must be bounded"
        );
        let cwd = c["cwd"].as_str().context("Invalid check cwd")?;
        if cwd != "." {
            relative(cwd)?
        }
        result.push(c);
    }
    Ok(json!(result))
}
pub fn environment(cmd: &mut Command) {
    let keys = [
        "HOME",
        "PATH",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "XDG_CACHE_HOME",
        "CARGO_TARGET_DIR",
        "GRADLE_USER_HOME",
        "ZIG_GLOBAL_CACHE_DIR",
        "ZIG_LOCAL_CACHE_DIR",
        "PYTHONDONTWRITEBYTECODE",
    ];
    let env = std::env::vars_os()
        .filter(|(k, _)| keys.contains(&k.to_string_lossy().as_ref()))
        .collect::<Vec<_>>();
    cmd.env_clear().envs(env);
    let paths = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|p| p.is_absolute())
        .collect::<Vec<_>>();
    cmd.env("PATH", std::env::join_paths(paths).unwrap_or_default());
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
}
fn cwd(root: &Path, relative: &str) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let path = root.join(relative).canonicalize()?;
    ensure!(
        path.is_dir() && path.starts_with(root),
        "Check cwd escaped candidate"
    );
    Ok(path)
}
fn storage() -> Result<()> {
    let policy = Path::new("/mnt/fastbuild/INFO.md");
    if policy.parent().unwrap().exists() {
        fs::read_to_string(policy)?;
        let status = Command::new("mountpoint")
            .arg("-q")
            .arg("/mnt/fastbuild")
            .status()?;
        ensure!(
            status.success(),
            "Required /mnt/fastbuild mount unavailable"
        );
    }
    Ok(())
}
pub fn execute(check: &Value, checkout: &Path, log: &Path) -> Result<Value> {
    let args = array(&check["argv"]);
    let mut cmd = Command::new(args[0].as_str().unwrap());
    cmd.args(args.iter().skip(1).map(|v| v.as_str().unwrap()))
        .current_dir(cwd(checkout, &text(check, "cwd"))?)
        .stdin(Stdio::null());
    // One Unix pipe keeps stdout/stderr ordering and permits process-group cleanup.
    let (reader, writer) = std::os::unix::net::UnixStream::pair()?;
    cmd.stdout(Stdio::from(std::os::fd::OwnedFd::from(writer.try_clone()?)))
        .stderr(Stdio::from(std::os::fd::OwnedFd::from(writer)));
    environment(&mut cmd);
    process_group(&mut cmd);
    let mut child = cmd.spawn()?;
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let retained = bytes.clone();
    let limited = Arc::new(AtomicBool::new(false));
    let flag = limited.clone();
    let limit = check["output_limit"].as_u64().unwrap() as usize;
    reader.set_read_timeout(Some(Duration::from_millis(50)))?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_read = stop.clone();
    let worker = thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0; 65536];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut data = retained.lock().unwrap();
                    let keep = n.min(limit - data.len());
                    data.extend_from_slice(&buf[..keep]);
                    if keep < n {
                        flag.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if stop_read.load(Ordering::SeqCst) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let start = Instant::now();
    let timeout = Duration::from_secs_f64(check["timeout"].as_f64().unwrap());
    let mut timed_out = false;
    let status = loop {
        if limited.load(Ordering::SeqCst) {
            kill_group(child.id());
            break child.wait()?;
        }
        if start.elapsed() > timeout {
            timed_out = true;
            kill_group(child.id());
            break child.wait()?;
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(10));
    };
    kill_group(child.id());
    stop.store(true, Ordering::SeqCst);
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("Check output reader failed"))?;
    let data = bytes.lock().unwrap();
    write_new(log, &data, 0o600)?;
    use std::os::unix::process::ExitStatusExt;
    let code = status.code().unwrap_or(-status.signal().unwrap_or(1));
    Ok(
        json!({"name":check["name"],"argv":check["argv"],"returncode":code,"timed_out":timed_out,"output_limited":limited.load(Ordering::SeqCst),"log":log,"output_sha256":sha(&data)}),
    )
}
pub fn run(spec: &Value) -> Result<Value> {
    let mut receipt = json!({"token":spec["token"],"snapshot":spec["snapshot"],"spec_sha256":digest(spec),"results":[],"passed":false});
    let outcome = (|| -> Result<()> {
        storage()?;
        let checks = normalise(&spec["checks"])?;
        for c in array(&checks) {
            let result = execute(
                &c,
                Path::new(&text(spec, "checkout")),
                &PathBuf::from(text(spec, "directory")).join(format!("{}.log", text(&c, "name"))),
            )?;
            let passed = result["returncode"] == 0
                && result["timed_out"] == false
                && result["output_limited"] == false;
            receipt["results"].as_array_mut().unwrap().push(result);
            if !passed {
                break;
            }
        }
        receipt["passed"] = json!(
            array(&receipt["results"]).len() == array(&checks).len()
                && array(&receipt["results"])
                    .iter()
                    .all(|r| r["returncode"] == 0
                        && !r["timed_out"].as_bool().unwrap_or(true)
                        && !r["output_limited"].as_bool().unwrap_or(true))
        );
        Ok(())
    })();
    if outcome.is_err() {
        receipt["error"] =
            json!("Approved check execution unavailable; inspect private helper evidence");
    }
    atomic_json(
        &PathBuf::from(text(spec, "directory")).join("receipt.json"),
        &receipt,
    )?;
    Ok(receipt)
}
#[derive(Default)]
pub struct Runner {
    children: Vec<Child>,
    executable: Option<PathBuf>,
}
impl Runner {
    pub fn with_executable(executable: PathBuf) -> Self {
        Self {
            children: Vec::new(),
            executable: Some(executable),
        }
    }
    pub fn reap(&mut self) {
        self.children
            .retain_mut(|c| c.try_wait().ok().flatten().is_none());
    }
    pub fn start(
        &mut self,
        dir: &Path,
        checkout: &Path,
        checks: &Value,
        snapshot: &str,
    ) -> Result<Value> {
        self.reap();
        let checks = normalise(checks)?;
        for c in array(&checks) {
            cwd(checkout, &text(&c, "cwd"))?;
        }
        let dir = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir()?.join(dir)
        };
        fresh_destination(&dir)?;
        private_dir(&dir)?;
        let spec = json!({"token":uuid::Uuid::new_v4().to_string(),"checkout":checkout.canonicalize()?,"checks":checks,"snapshot":snapshot,"directory":dir});
        let path = dir.join("spec.json");
        write_new(&path, canonical(&spec).as_bytes(), 0o600)?;
        let log = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join("helper.log"))?;
        let mut cmd = Command::new(self.executable.clone().unwrap_or(std::env::current_exe()?));
        cmd.arg("check-helper")
            .arg(&path)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        environment(&mut cmd);
        process_group(&mut cmd);
        let child = cmd.spawn()?;
        let pid = child.id();
        self.children.push(child);
        Ok(
            json!({"spec":path,"receipt":dir.join("receipt.json"),"token":spec["token"],"spec_sha256":digest(&spec),"snapshot":snapshot,"pid":pid,"deadline":now()+array(&checks).iter().map(|c|c["timeout"].as_f64().unwrap()).sum::<f64>()+30.0}),
        )
    }
}
pub fn inspect(h: &Value) -> Result<Option<Value>> {
    let path = PathBuf::from(text(h, "receipt"));
    if !path.exists() {
        return Ok(None);
    }
    let mut receipt: Value = serde_json::from_slice(&regular(&path, 1048576)?)?;
    let spec = read_json(Path::new(&text(h, "spec")))?;
    ensure!(
        digest(&spec) == h["spec_sha256"]
            && receipt["token"] == h["token"]
            && receipt["spec_sha256"] == h["spec_sha256"]
            && receipt["snapshot"] == h["snapshot"]
            && receipt["results"].is_array()
            && array(&receipt["results"]).len() <= array(&spec["checks"]).len(),
        "Check receipt does not match launch/snapshot"
    );
    for (expected, result) in array(&spec["checks"])
        .iter()
        .zip(array(&receipt["results"]))
    {
        let log =
            PathBuf::from(text(&spec, "directory")).join(format!("{}.log", text(expected, "name")));
        ensure!(
            result["name"] == expected["name"]
                && result["argv"] == expected["argv"]
                && result["log"] == json!(log)
                && sha(&regular(&log, 1048576)?) == result["output_sha256"]
                && result["returncode"].is_i64(),
            "Check command or output evidence changed"
        );
    }
    receipt["passed"] = json!(
        receipt.get("error").is_none()
            && array(&receipt["results"]).len() == array(&spec["checks"]).len()
            && array(&receipt["results"])
                .iter()
                .all(|r| r["returncode"] == 0
                    && !r["timed_out"].as_bool().unwrap_or(true)
                    && !r["output_limited"].as_bool().unwrap_or(true))
    );
    Ok(Some(receipt))
}
