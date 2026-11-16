//! Additive global registration; never opens supervisor state or project configuration.
use crate::common::regular;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

const ASSETS: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!("../ui/teem-progress/package.json"),
    ),
    ("server.ts", include_str!("../ui/teem-progress/server.ts")),
    ("tui.tsx", include_str!("../ui/teem-progress/tui.tsx")),
    ("enable.mjs", include_str!("../ui/teem-progress/enable.mjs")),
    (
        "progress.mjs",
        include_str!("../ui/teem-progress/progress.mjs"),
    ),
];

fn safe_path(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir)),
        "Use an absolute path without traversal: {}",
        path.display()
    );
    for part in path.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(m) => ensure!(
                !m.file_type().is_symlink(),
                "Refusing symlink {}; select a real installation/configuration path",
                part.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn env_path(key: &str, fallback: impl FnOnce() -> Result<PathBuf>) -> Result<PathBuf> {
    let path = match std::env::var_os(key) {
        Some(value) => PathBuf::from(value),
        None => fallback()?,
    };
    safe_path(&path).map_err(|error| anyhow::anyhow!("Invalid {key}: {error:#}"))?;
    Ok(path)
}

fn directory(path: &Path) -> Result<()> {
    safe_path(path)?;
    // Never chmod existing user directories.
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    ensure!(
        fs::metadata(path)?.is_dir(),
        "Not a directory: {}",
        path.display()
    );
    Ok(())
}

fn snapshot(path: &Path) -> Result<Option<Vec<u8>>> {
    safe_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(m) => {
            ensure!(
                m.is_file(),
                "Refusing non-regular configuration file {}; move it explicitly before installing",
                path.display()
            );
            Ok(Some(regular(path, 16 * 1024 * 1024)?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn atomic(path: &Path, data: &[u8], mode: u32) -> Result<()> {
    safe_path(path)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(metadata.is_file(), "Not a regular file: {}", path.display());
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("Missing parent")?)?;
    file.write_all(data)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub fn install(config: &Path) -> Result<()> {
    let home = || env_path("HOME", || anyhow::bail!("Set HOME for installation"));
    let config_home = env_path("XDG_CONFIG_HOME", || Ok(home()?.join(".config")))?;
    let data_home = env_path("XDG_DATA_HOME", || Ok(home()?.join(".local/share")))?;
    safe_path(config)?;
    let config_dir = config_home.join("opencode");
    let json_path = config_dir.join("opencode.json");
    let jsonc_path = config_dir.join("opencode.jsonc");
    let plain = snapshot(&json_path)?;
    let commented = snapshot(&jsonc_path)?;
    ensure!(
        plain.is_none() || commented.is_none(),
        "Both {} and {} exist; consolidate them explicitly before installing Teem",
        json_path.display(),
        jsonc_path.display()
    );
    let (path, before) = if commented.is_some() {
        (jsonc_path, commented)
    } else {
        (json_path, plain)
    };
    let original = std::str::from_utf8(before.as_deref().unwrap_or(b"{}\n"))?;
    let owned = data_home.join("teem/opencode");
    // CLI precedence is host-owned. Refuse duplicate or disabled registrations.
    let mut cli_snapshots = Vec::new();
    for name in ["cli.json", "cli.jsonc"] {
        let cli_path = config_dir.join(name);
        let bytes = snapshot(&cli_path)?;
        if let Some(bytes) = &bytes {
            let cli = Document::new(std::str::from_utf8(bytes)?).with_context(|| {
                format!("Fix {} explicitly before installation", cli_path.display())
            })?;
            if let Some(plugins) = cli.value.get("plugins") {
                let plugins = plugins.as_array().context("CLI plugins must be an array")?;
                ensure!(
                    !plugins.iter().any(|p| teem_entry(p, &owned)),
                    "Existing Teem registration/disable entry in {}; consolidate its custom options into global opencode.json/jsonc explicitly before installation; CLI configuration was not changed",
                    cli_path.display()
                );
            }
        }
        cli_snapshots.push((cli_path, bytes));
    }
    let executable = regular(&std::env::current_exe()?, 512 * 1024 * 1024)?;
    let mut identity = Sha256::new();
    identity.update(&executable);
    for (name, data) in ASSETS {
        identity.update(name.as_bytes());
        identity.update(data.as_bytes());
    }
    let bundle = owned.join(format!("{:x}", identity.finalize()));
    let package = bundle.join("ui/teem-progress");
    let installed = bundle.join("bin/teem");
    let output = registration(original, &package, &installed, config, &owned).map_err(|e| {
        anyhow::anyhow!("Cannot register Teem in {}: {e:#}. Fix the document explicitly; nothing was overwritten", path.display())
    })?;
    let local_binary = std::env::current_dir()?.join("bin/teem");
    safe_path(&local_binary)?;
    safe_path(&bundle)?;
    if bundle.exists() {
        safe_path(&installed)?;
        ensure!(
            fs::symlink_metadata(&installed)?.is_file(),
            "Invalid installed executable: {}",
            installed.display()
        );
        ensure!(
            fs::metadata(&installed)?.permissions().mode() & 0o100 != 0,
            "Installed Teem executable is not owner-executable: {}; restore its execute permission explicitly before retrying",
            installed.display()
        );
        ensure!(
            regular(&installed, 512 * 1024 * 1024)? == executable,
            "Installed Teem bundle was modified: {}",
            bundle.display()
        );
        for (name, data) in ASSETS {
            safe_path(&package.join(name))?;
            ensure!(
                fs::symlink_metadata(package.join(name))?.is_file()
                    && regular(&package.join(name), 1024 * 1024)? == data.as_bytes(),
                "Installed Teem asset was modified: {}",
                package.join(name).display()
            );
        }
    } else {
        directory(&owned)?;
        let stage = tempfile::tempdir_in(&owned)?;
        let ui = stage.path().join("ui/teem-progress");
        directory(&ui)?;
        directory(&stage.path().join("bin"))?;
        atomic(&stage.path().join("bin/teem"), &executable, 0o755)?;
        for (name, data) in ASSETS {
            atomic(&ui.join(name), data.as_bytes(), 0o600)?;
        }
        fs::rename(stage.path(), &bundle)?;
    }
    directory(local_binary.parent().unwrap())?;
    atomic(&local_binary, &executable, 0o755)?;
    directory(&config_dir)?;
    safe_path(&config_dir.join("teem-install.lock"))?;
    let lock = crate::common::open_lock(&config_dir.join("teem-install.lock"))?;
    use std::os::fd::AsRawFd;
    // SAFETY: lock owns the descriptor until this function returns.
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0,
        "Cannot lock Teem installation"
    );
    ensure!(
        snapshot(&path)? == before,
        "{} changed during installation; retry after the other editor finishes",
        path.display()
    );
    let other = if path.extension().is_some_and(|s| s == "jsonc") {
        config_dir.join("opencode.json")
    } else {
        config_dir.join("opencode.jsonc")
    };
    ensure!(
        snapshot(&other)?.is_none(),
        "Another OpenCode config appeared; consolidate it explicitly"
    );
    for (cli_path, bytes) in cli_snapshots {
        ensure!(
            snapshot(&cli_path)? == bytes,
            "{} changed during installation; retry after the other editor finishes",
            cli_path.display()
        );
    }
    if output.as_bytes() != original.as_bytes() || before.is_none() {
        let mode = if before.is_some() {
            fs::metadata(&path)?.permissions().mode() & 0o777
        } else {
            0o600
        };
        atomic(&path, output.as_bytes(), mode)?;
    }
    println!(
        "Installed shared Teem agent and native plugin in {} (no activation; existing identities/defaults preserved)",
        path.display()
    );
    Ok(())
}

// Preserve JSONC bytes outside registration spans. Mask comments/trailing commas
// only in the validation copy, preserving byte offsets for additive edits.
struct Document<'a> {
    raw: &'a str,
    masked: Vec<u8>,
    value: Value,
    spans: BTreeMap<Vec<String>, (usize, usize)>,
    edits: Vec<(usize, usize, String)>,
}

impl<'a> Document<'a> {
    fn new(raw: &'a str) -> Result<Self> {
        let mut masked = raw.as_bytes().to_vec();
        let mut i = 0;
        while i < masked.len() {
            if masked[i] == b'"' {
                i = string_end(&masked, i)?;
            } else if masked.get(i..i + 2) == Some(b"//") {
                while i < masked.len() && masked[i] != b'\n' {
                    masked[i] = b' ';
                    i += 1;
                }
            } else if masked.get(i..i + 2) == Some(b"/*") {
                masked[i..i + 2].fill(b' ');
                i += 2;
                while i + 1 < masked.len() && &masked[i..i + 2] != b"*/" {
                    masked[i] = b' ';
                    i += 1;
                }
                ensure!(i + 1 < masked.len(), "Unterminated JSONC comment");
                masked[i..i + 2].fill(b' ');
                i += 2;
            } else {
                i += 1;
            }
        }
        let comments_masked = masked.clone();
        i = 0;
        while i < masked.len() {
            if masked[i] == b'"' {
                i = string_end(&masked, i)?;
                continue;
            }
            if masked[i] == b','
                // A trailing comma must follow a value, not an empty container
                // or another comma. Serde validates the complete value below.
                && comments_masked[..i]
                    .iter()
                    .rev()
                    .find(|b| !b.is_ascii_whitespace())
                    .is_some_and(|b| matches!(b, b'"' | b'}' | b']' | b'0'..=b'9' | b'e' | b'l'))
                && masked[i + 1..]
                    .iter()
                    .find(|b| !b.is_ascii_whitespace())
                    .is_some_and(|b| matches!(b, b'}' | b']'))
            {
                masked[i] = b' ';
            }
            i += 1;
        }
        let value: Value =
            serde_json::from_slice(&masked).context("Use a valid JSON/JSONC object")?;
        ensure!(
            value.is_object(),
            "OpenCode configuration must be an object"
        );
        let mut spans = BTreeMap::new();
        walk(&masked, &mut 0, &mut Vec::new(), &mut spans)?;
        Ok(Self {
            raw,
            masked: comments_masked,
            value,
            spans,
            edits: Vec::new(),
        })
    }

    fn append(&mut self, path: &[&str], content: String) -> Result<()> {
        let key = path.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let (start, end) = *self
            .spans
            .get(&key)
            .context("Missing configuration container")?;
        let last = self.masked[start + 1..end - 1]
            .iter()
            .rev()
            .find(|b| !b.is_ascii_whitespace());
        let comma = if last.is_some_and(|b| *b != b',') {
            ","
        } else {
            ""
        };
        self.edits
            .push((end - 1, end - 1, format!("{comma}\n{content}\n")));
        Ok(())
    }

    fn finish(mut self) -> Result<String> {
        self.edits.sort_by_key(|e| std::cmp::Reverse(e.0));
        let mut result = self.raw.to_owned();
        for (start, end, text) in self.edits {
            result.replace_range(start..end, &text);
        }
        Document::new(&result)?;
        Ok(result)
    }
}

fn string_end(bytes: &[u8], start: usize) -> Result<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    anyhow::bail!("Unterminated JSON string")
}

fn whitespace(bytes: &[u8], i: &mut usize) {
    while bytes.get(*i).is_some_and(u8::is_ascii_whitespace) {
        *i += 1;
    }
}

fn walk(
    bytes: &[u8],
    i: &mut usize,
    path: &mut Vec<String>,
    spans: &mut BTreeMap<Vec<String>, (usize, usize)>,
) -> Result<()> {
    whitespace(bytes, i);
    let start = *i;
    match bytes[*i] {
        b'{' | b'[' => {
            let object = bytes[*i] == b'{';
            let close = if object { b'}' } else { b']' };
            *i += 1;
            whitespace(bytes, i);
            let mut index = 0;
            while bytes[*i] != close {
                let key = if object {
                    let end = string_end(bytes, *i)?;
                    let key: String = serde_json::from_slice(&bytes[*i..end])?;
                    *i = end;
                    whitespace(bytes, i);
                    *i += 1; // colon, already validated by serde_json
                    key
                } else {
                    index.to_string()
                };
                path.push(key);
                walk(bytes, i, path, spans)?;
                path.pop();
                index += 1;
                whitespace(bytes, i);
                if bytes[*i] == b',' {
                    *i += 1;
                    whitespace(bytes, i);
                }
            }
            *i += 1;
        }
        b'"' => *i = string_end(bytes, *i)?,
        _ => {
            while bytes
                .get(*i)
                .is_some_and(|b| !b.is_ascii_whitespace() && !b",]}".contains(b))
            {
                *i += 1;
            }
        }
    }
    ensure!(
        spans.insert(path.clone(), (start, *i)).is_none(),
        "Duplicate JSON key at {}; consolidate explicitly",
        path.join(".")
    );
    Ok(())
}

fn registration(
    raw: &str,
    package: &Path,
    executable: &Path,
    config: &Path,
    owned: &Path,
) -> Result<String> {
    let mut d = Document::new(raw)?;
    let mut template: Value = serde_json::from_str(include_str!("../examples/pilot-foreman.json"))?;
    // Canonical global chats use authenticated native tools, not a fixed MCP owner.
    template["agents"]["teem_foreman"]["system"] = json!(
        template["agents"]["teem_foreman"]["system"]
            .as_str()
            .context("Missing shipped system prompt")?
            .replace("teem_foreman_", "teem_")
            .replace("foreman_", "teem_")
            .replace("You are the Teem Foreman", "You are Teem")
            .replace("Foreman", "Teem")
    );
    template["agents"]["teem_foreman"]["description"] = json!(
        "Teem: deliver authorised tasks through managed workers, checks and independent reviews"
    );
    let profile = &template["agents"]["teem_foreman"];
    ensure!(
        profile.is_object(),
        "Missing shipped Foreman agent template"
    );
    let mut root = Vec::new();
    if let Some(agents) = d.value.get("agents") {
        ensure!(agents.is_object(), "agents must be an object");
        if let Some(existing) = agents.get("teem") {
            ensure!(existing.is_object(), "agents.teem must be an object");
        } else {
            d.append(
                &["agents"],
                format!("\"teem\":{}", serde_json::to_string(profile)?),
            )?;
        }
    } else {
        root.push(format!(
            "\"agents\":{{\"teem\":{}}}",
            serde_json::to_string(profile)?
        ));
    }
    let options = json!({"executable":executable,"config":config});
    let entry = json!({"package":package,"options":options});
    if let Some(plugins) = d.value.get("plugins") {
        let plugins = plugins.as_array().context("plugins must be an array")?;
        let matches = plugins
            .iter()
            .enumerate()
            .filter(|(_, p)| teem_entry(p, owned))
            .map(|(i, p)| (i, p.clone()))
            .collect::<Vec<_>>();
        ensure!(
            matches.len() <= 1,
            "Multiple Teem plugin registrations; consolidate explicitly"
        );
        if let Some((index, existing)) = matches.into_iter().next() {
            let name = existing
                .as_str()
                .or_else(|| existing["package"].as_str())
                .unwrap_or("")
                .to_owned();
            ensure!(
                !name.starts_with('-'),
                "Teem plugin is disabled; resolve explicitly before installation"
            );
            if name != "local-teem-progress" {
                safe_path(Path::new(&name)).context("Invalid existing Teem package path")?;
            }
            ensure!(
                Path::new(&name)
                    .file_name()
                    .is_some_and(|s| s == "teem-progress")
                    || name == "local-teem-progress",
                "Unsupported Teem entry point {name}; consolidate the server/TUI package registration explicitly"
            );
            let index = index.to_string();
            if existing.is_string() {
                let package = if Path::new(&name).starts_with(owned) {
                    json!(package)
                } else {
                    json!(name)
                };
                let (start, end) = d.spans[&vec!["plugins".into(), index]];
                d.edits.push((
                    start,
                    end,
                    serde_json::to_string(&json!({"package":package,"options":options}))?,
                ));
            } else {
                if Path::new(&name).starts_with(owned) && existing["package"] != json!(package) {
                    let (start, end) =
                        d.spans[&vec!["plugins".into(), index.clone(), "package".into()]];
                    d.edits.push((start, end, serde_json::to_string(package)?));
                }
                if let Some(opts) = existing.get("options") {
                    let opts = opts
                        .as_object()
                        .context("Teem plugin options must be an object")?;
                    if let Some(state) = opts.get("stateFile") {
                        let state = state
                            .as_str()
                            .context("Teem stateFile must be a path string")?;
                        safe_path(Path::new(state)).context("Invalid Teem stateFile option")?;
                    }
                    if let Some(native) = opts.get("nativeTodos") {
                        ensure!(native.is_boolean(), "Teem nativeTodos must be a boolean");
                    }
                    let mut missing = Vec::new();
                    for (key, value) in options.as_object().unwrap() {
                        if let Some(old) = opts.get(key) {
                            let old_path = old
                                .as_str()
                                .with_context(|| format!("Teem {key} must be a path string"))?;
                            safe_path(Path::new(old_path))
                                .with_context(|| format!("Invalid Teem {key} option"))?;
                            if key == "executable"
                                && Path::new(old_path).starts_with(owned)
                                && old != value
                            {
                                let (start, end) = d.spans[&vec![
                                    "plugins".into(),
                                    index.clone(),
                                    "options".into(),
                                    key.clone(),
                                ]];
                                d.edits.push((start, end, serde_json::to_string(value)?));
                            }
                        } else {
                            missing.push(format!(
                                "{}:{}",
                                serde_json::to_string(key)?,
                                serde_json::to_string(value)?
                            ));
                        }
                    }
                    if !missing.is_empty() {
                        d.append(&["plugins", &index, "options"], missing.join(",\n"))?;
                    }
                } else {
                    d.append(
                        &["plugins", &index],
                        format!("\"options\":{}", serde_json::to_string(&options)?),
                    )?;
                }
            }
        } else {
            d.append(&["plugins"], serde_json::to_string(&entry)?)?;
        }
    } else {
        root.push(format!("\"plugins\":[{}]", serde_json::to_string(&entry)?));
    }
    if !root.is_empty() {
        d.append(&[], root.join(",\n"))?;
    }
    d.finish()
}

fn teem_entry(entry: &Value, owned: &Path) -> bool {
    let name = entry
        .as_str()
        .or_else(|| entry["package"].as_str())
        .unwrap_or("");
    let name = name.strip_prefix('-').unwrap_or(name);
    matches!(
        name,
        "local-teem-progress" | "local.teem.native" | "local.teem.progress"
    ) || Path::new(name).starts_with(owned)
        || Path::new(name)
            .components()
            .any(|c| c.as_os_str() == "teem-progress")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register(raw: &str) -> Result<String> {
        registration(
            raw,
            Path::new("/data/teem/opencode/new/ui/teem-progress"),
            Path::new("/data/teem/opencode/new/bin/teem"),
            Path::new("/herdr/config.toml"),
            Path::new("/data/teem/opencode"),
        )
    }

    #[test]
    fn comments_custom_paths_and_unrelated_values_survive_repeat() {
        let raw = r#"{
          // user global policy
          "default_agent":"build", "url":"https://example.test/*value*/,]",
          "agents":{"teem_foreman":{"system":"legacy","hidden":false,},},
          "plugins":["unrelated",{"package":"/custom/teem-progress","options":{
            // custom bridge
            "executable":"/custom/bin/teem","config":"/custom/config.toml",
            "stateFile":"/private/progress.json","extra":true,
          },}],
          "mcp":{"servers":{}},
        }"#;
        let output = register(raw).unwrap();
        for text in [
            "// user global policy",
            "// custom bridge",
            "https://example.test/*value*/,]",
        ] {
            assert!(output.contains(text));
        }
        let before = Document::new(raw).unwrap().value;
        let parsed = Document::new(&output).unwrap().value;
        for key in ["plugins", "mcp", "default_agent", "url"] {
            assert_eq!(parsed[key], before[key]);
        }
        assert_eq!(
            parsed["agents"]["teem_foreman"],
            before["agents"]["teem_foreman"]
        );
        assert!(parsed["agents"]["teem"].is_object());
        assert_eq!(register(&output).unwrap(), output);
    }

    #[test]
    fn trailing_commas_require_a_preceding_value() {
        for raw in [
            "{,}",
            r#"{"permissions":[,]}"#,
            r#"{"permissions":{/* empty */,}}"#,
            r#"{"permissions":[/* empty */,/* end */]}"#,
            r#"{"permissions":[1,,]}"#,
            r#"{"permissions": [[], [,]]}"#,
        ] {
            assert!(register(raw).is_err(), "accepted {raw}");
        }
        let raw = r#"{"values":["text",{},[],0,-1,1.5,1e2,true,false,null,/* end */],}"#;
        let output = register(raw).unwrap();
        assert_eq!(
            Document::new(&output).unwrap().value["values"],
            json!(["text", {}, [], 0, -1, 1.5, 1e2, true, false, null])
        );
        assert!(output.contains(raw.trim_end_matches('}')));
        assert_eq!(register(&output).unwrap(), output);
    }

    #[test]
    fn malformed_jsonc_and_nested_duplicate_keys_refuse() {
        for raw in [
            r#"{"agents":{"teem":{},"teem":{}}}"#,
            "{/* unfinished",
            "{\"a\":1,,}",
            r#"{"plugins":[{"package":"/custom/teem-progress","options":null}]}"#,
        ] {
            assert!(register(raw).is_err(), "accepted {raw}");
        }
    }
}
