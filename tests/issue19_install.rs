//! Installer children have no inherited operator environment, state or credentials.
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn path(&self, path: &str) -> PathBuf {
        self.0.path().join(path)
    }
    fn command(&self, executable: &Path) -> Command {
        let mut cmd = Command::new(executable);
        cmd.env_clear()
            .current_dir(self.0.path())
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("HERDR_PLUGIN_CONFIG_DIR", self.path("herdr-config"));
        cmd
    }
    fn run(&self, command: &str) -> Output {
        self.command(Path::new(env!("CARGO_BIN_EXE_teem")))
            .arg(command)
            .output()
            .unwrap()
    }
    fn install(&self) -> Output {
        self.run("install-plugin")
    }
    fn write(&self, name: &str, data: impl AsRef<[u8]>) {
        let path = self.path(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
    fn global(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path("config/opencode/opencode.json")).unwrap())
            .unwrap()
    }
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

#[test]
fn fresh_shared_install_repeat_and_unconfigured_native_bootstrap() {
    let f = Fixture::new();
    success(f.install());
    let global = f.global();
    let mut template: Value =
        serde_json::from_str(include_str!("../examples/pilot-teem.json")).unwrap();
    template["agents"]["teem"]["system"] = json!(format!(
        "{}\n\n{}",
        template["agents"]["teem"]["system"].as_str().unwrap(),
        include_str!("../docs/AUTONOMOUS_DELIVERY.md")
    ));
    assert!(
        !global["agents"]["teem"]["system"]
            .as_str()
            .unwrap()
            .contains("foreman_")
    );
    assert_eq!(global["agents"]["teem"], template["agents"]["teem"]);
    assert!(global["agents"].get("teem_foreman").is_none());
    for key in [
        "default_agent",
        "warming",
        "experimental",
        "model",
        "permissions",
        "mcp",
    ] {
        assert!(global.get(key).is_none(), "unexpected global {key}");
    }
    let plugin = &global["plugins"][0];
    let package = Path::new(plugin["package"].as_str().unwrap());
    let executable = Path::new(plugin["options"]["executable"].as_str().unwrap());
    assert!(package.starts_with(f.path("data/teem/opencode")));
    for file in [
        "package.json",
        "server.ts",
        "tui.tsx",
        "enable.mjs",
        "progress.mjs",
    ] {
        assert!(package.join(file).is_file());
    }
    assert_eq!(
        plugin["options"]["config"],
        json!(f.path("herdr-config/config.toml"))
    );
    // Global registration must survive disappearance of Herdr's build cwd binary.
    fs::remove_file(f.path("bin/teem")).unwrap();
    let catalog = success(f.command(executable).arg("native-tools").output().unwrap());
    assert_eq!(catalog, teem::enable::tools());
    assert_eq!(
        success(f.command(executable).arg("native-setup").output().unwrap()),
        json!({"stateFile":null})
    );
    assert!(!f.path("herdr-config").exists());
    assert!(!f.path("state").exists());
    let unavailable = f.command(executable).arg("native-call").output().unwrap();
    assert!(!unavailable.status.success());
    assert!(String::from_utf8_lossy(&unavailable.stderr).contains("Configure Teem at"));
    let before = fs::read(f.path("config/opencode/opencode.json")).unwrap();
    success(f.install());
    assert_eq!(
        fs::read(f.path("config/opencode/opencode.json")).unwrap(),
        before
    );
    assert_eq!(
        fs::read_dir(f.path("data/teem/opencode")).unwrap().count(),
        1
    );
}

#[test]
fn upgrade_preserves_custom_options_legacy_identity_and_repo_overrides() {
    let f = Fixture::new();
    let old = f.path("data/teem/opencode/previous");
    let custom = json!({"system":"legacy policy","model":"custom/model","hidden":false});
    let original = json!({
        "default_agent":"build", "warming":true, "model":"global/model",
        "permissions":[{"action":"*","effect":"deny"}],
        "agents":{"build":{"color":"#ffffff"},"teem_foreman":custom,"other":{}},
        "mcp":{"servers":{"personal":{"command":["my-mcp"]}}},
        "plugins":["unrelated",{"package":old.join("ui/teem-progress"),"options":{
            "executable":old.join("bin/teem"),"config":f.path("custom/config.toml"),
            "stateFile":f.path("custom/progress.json"),"custom":true}}]
    });
    f.write(
        "config/opencode/opencode.json",
        serde_json::to_vec(&original).unwrap(),
    );
    let repo =
        b"{\"default_agent\":\"teem_foreman\",\"agents\":{\"teem\":{\"model\":\"repo/model\"}}}\n";
    f.write("repo/opencode.json", repo);
    success(f.install());
    let installed = f.global();
    for key in ["default_agent", "warming", "model", "permissions", "mcp"] {
        assert_eq!(installed[key], original[key]);
    }
    assert_eq!(installed["agents"]["teem_foreman"], custom);
    assert_eq!(installed["agents"]["build"], original["agents"]["build"]);
    assert_eq!(installed["agents"]["other"], original["agents"]["other"]);
    assert_eq!(installed["plugins"][0], "unrelated");
    assert_ne!(
        installed["plugins"][1]["package"],
        original["plugins"][1]["package"]
    );
    let opts = &installed["plugins"][1]["options"];
    assert_ne!(
        opts["executable"],
        original["plugins"][1]["options"]["executable"]
    );
    for key in ["config", "stateFile", "custom"] {
        assert_eq!(opts[key], original["plugins"][1]["options"][key]);
    }
    assert_eq!(
        fs::read(f.path("repo/opencode.json")).unwrap(),
        repo.to_vec()
    );
    assert!(!f.path("custom").exists());
    success(f.install());
    assert_eq!(f.global(), installed);
}

#[test]
fn invalid_conflicting_and_symlink_configs_are_not_overwritten() {
    let f = Fixture::new();
    for raw in [
        "{bad",
        "[]",
        "{\"agents\":[]}",
        "{\"plugins\":{}}",
        "{\"agents\":{\"teem\":false}}",
        "{\"model\":1,\"model\":2}",
        "{\"plugins\":[\"-/custom/teem-progress\"]}",
        "{\"plugins\":[\"/a/teem-progress\",\"/b/teem-progress\"]}",
        "{\"plugins\":[\"/custom/teem-progress/server.ts\"]}",
        "{\"plugins\":[{\"package\":\"/a/teem-progress\",\"options\":{\"config\":\"relative\"}}]}",
        "{\"plugins\":[{\"package\":\"/a/teem-progress\",\"options\":{\"nativeTodos\":null}}]}",
    ] {
        f.write("config/opencode/opencode.json", raw);
        assert!(!f.install().status.success(), "accepted {raw}");
        assert_eq!(
            fs::read_to_string(f.path("config/opencode/opencode.json")).unwrap(),
            raw
        );
        assert!(!f.path("bin").exists());
        assert!(!f.path("data").exists());
    }
    f.write("config/opencode/opencode.json", "{}");
    f.write("config/opencode/opencode.jsonc", "{} // second document");
    assert!(!f.install().status.success());
    fs::remove_file(f.path("config/opencode/opencode.jsonc")).unwrap();
    fs::remove_file(f.path("config/opencode/opencode.json")).unwrap();
    f.write("outside.json", "{\"private\":true}");
    symlink(
        f.path("outside.json"),
        f.path("config/opencode/opencode.json"),
    )
    .unwrap();
    assert!(!f.install().status.success());
    assert_eq!(
        fs::read_to_string(f.path("outside.json")).unwrap(),
        "{\"private\":true}"
    );
    fs::remove_file(f.path("config/opencode/opencode.json")).unwrap();
    fs::remove_dir(f.path("config/opencode")).unwrap();
    fs::create_dir(f.path("outside-dir")).unwrap();
    symlink(f.path("outside-dir"), f.path("config/opencode")).unwrap();
    assert!(!f.install().status.success());
    assert_eq!(fs::read_dir(f.path("outside-dir")).unwrap().count(), 0);
}

#[test]
fn commas_in_empty_containers_refuse_without_writes() {
    for name in ["opencode.json", "opencode.jsonc"] {
        for raw in [
            "{,}",
            r#"{"permissions":[,]}"#,
            r#"{"permissions":{,}}"#,
            r#"{"permissions":[/* empty */,/* end */]}"#,
            r#"{"permissions":{/* empty */,/* end */}}"#,
            r#"{"permissions":[1,,]}"#,
        ] {
            let f = Fixture::new();
            let path = format!("config/opencode/{name}");
            f.write(&path, raw);
            let output = f.install();
            assert!(!output.status.success(), "accepted {name}: {raw}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("Fix the document explicitly")
            );
            assert_eq!(fs::read_to_string(f.path(&path)).unwrap(), raw);
            assert_eq!(fs::read_dir(f.path("config/opencode")).unwrap().count(), 1);
            for path in ["bin", "data", "state", "herdr-config"] {
                assert!(!f.path(path).exists(), "created {path} for {raw}");
            }
        }
    }
}

#[test]
fn unsafe_existing_plugin_paths_refuse_before_staging_or_rewriting() {
    for key in ["package", "executable", "config", "stateFile"] {
        for symlinked in [false, true] {
            let f = Fixture::new();
            let leaf = if key == "package" {
                "teem-progress"
            } else {
                "override"
            };
            let unsafe_path = if symlinked {
                f.write("custom/sentinel", "user data");
                symlink(f.path("custom"), f.path("linked")).unwrap();
                f.path("linked").join(leaf)
            } else {
                f.path("data/teem/opencode/../custom").join(leaf)
            };
            let mut plugin = json!({
                "package":f.path("data/teem/opencode/old/ui/teem-progress"),
                "options":{"executable":f.path("data/teem/opencode/old/bin/teem"),
                    "config":f.path("custom/config.toml"),"stateFile":f.path("custom/progress.json")}
            });
            if key == "package" {
                plugin["package"] = json!(unsafe_path);
            } else {
                plugin["options"][key] = json!(unsafe_path);
            }
            let original = serde_json::to_vec(&json!({"plugins":[plugin]})).unwrap();
            f.write("config/opencode/opencode.json", &original);
            let output = f.install();
            assert!(!output.status.success(), "accepted unsafe {key}");
            let error = String::from_utf8_lossy(&output.stderr);
            let reason = if symlinked { "symlink" } else { "traversal" };
            assert!(error.contains(reason), "{error}");
            assert_eq!(
                fs::read(f.path("config/opencode/opencode.json")).unwrap(),
                original
            );
            assert!(!f.path("data").exists());
            assert!(!f.path("bin").exists());
        }
    }
}

#[test]
fn asset_failure_and_tampering_preserve_global_config() {
    let f = Fixture::new();
    let original = b"{\"default_agent\":\"build\"}\n";
    f.write("config/opencode/opencode.json", original);
    f.write("data/teem", "blocking non-directory");
    assert!(!f.install().status.success());
    assert_eq!(
        fs::read(f.path("config/opencode/opencode.json")).unwrap(),
        original.to_vec()
    );
    fs::remove_file(f.path("data/teem")).unwrap();
    success(f.install());
    let global = f.global();
    let before = fs::read(f.path("config/opencode/opencode.json")).unwrap();
    let executable = Path::new(
        global["plugins"][0]["options"]["executable"]
            .as_str()
            .unwrap(),
    );
    fs::set_permissions(executable, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!f.install().status.success());
    assert_eq!(
        fs::metadata(executable).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
    let asset = Path::new(global["plugins"][0]["package"].as_str().unwrap()).join("enable.mjs");
    fs::write(&asset, "modified by operator").unwrap();
    assert!(!f.install().status.success());
    assert_eq!(fs::read_to_string(asset).unwrap(), "modified by operator");
    assert_eq!(
        fs::read(f.path("config/opencode/opencode.json")).unwrap(),
        before
    );
}

#[test]
fn catalog_is_static_and_setup_does_not_open_state() {
    let f = Fixture::new();
    f.write("herdr-config/config.toml", "not valid toml!");
    assert_eq!(success(f.run("native-tools")), teem::enable::tools());
    assert!(!f.run("native-setup").status.success());
    f.write(
        "herdr-config/config.toml",
        format!(
            "enabled=false\nstate_dir={:?}\n",
            f.path("private-state").to_str().unwrap()
        ),
    );
    assert_eq!(
        success(f.run("native-setup"))["stateFile"],
        json!(f.path("private-state/progress.json"))
    );
    assert!(!f.path("private-state").exists());
}

#[test]
fn jsonc_and_canonical_customizations_survive_byte_identical_repeat() {
    let f = Fixture::new();
    let original = "{\n// keep notes\n\"default_agent\":\"teem_foreman\",\n\"agents\":{\"teem\":{\"system\":\"custom\"},\"teem_foreman\":{\"hidden\":false}},\n\"plugins\":[\"unrelated\",],\n}\n";
    f.write("config/opencode/opencode.jsonc", original);
    f.write("config/opencode/cli.json", "{\"plugins\":[\"custom-tui\"]}");
    success(f.install());
    let before = fs::read_to_string(f.path("config/opencode/opencode.jsonc")).unwrap();
    assert!(before.contains("// keep notes"));
    assert!(before.contains("\"default_agent\":\"teem_foreman\""));
    assert!(before.contains("\"teem\":{\"system\":\"custom\"}"));
    assert!(before.contains("\"teem_foreman\":{\"hidden\":false}"));
    assert!(!f.path("config/opencode/opencode.json").exists());
    assert_eq!(
        fs::read_to_string(f.path("config/opencode/cli.json")).unwrap(),
        "{\"plugins\":[\"custom-tui\"]}"
    );
    success(f.install());
    assert_eq!(
        fs::read_to_string(f.path("config/opencode/opencode.jsonc")).unwrap(),
        before
    );
}

#[test]
fn home_fallback_and_explicit_config_are_fixture_owned() {
    let f = Fixture::new();
    success(
        f.command(Path::new(env!("CARGO_BIN_EXE_teem")))
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("HERDR_PLUGIN_CONFIG_DIR")
            .arg("--config")
            .arg(f.path("explicit/config.toml"))
            .arg("install-plugin")
            .output()
            .unwrap(),
    );
    let global: Value =
        serde_json::from_slice(&fs::read(f.path("home/.config/opencode/opencode.json")).unwrap())
            .unwrap();
    assert_eq!(
        global["plugins"][0]["options"]["config"],
        json!(f.path("explicit/config.toml"))
    );
    assert!(
        Path::new(global["plugins"][0]["package"].as_str().unwrap())
            .starts_with(f.path("home/.local/share/teem/opencode"))
    );
    assert!(!f.path("explicit").exists());
}

#[test]
fn legacy_cli_teem_registration_requires_explicit_consolidation() {
    let f = Fixture::new();
    for entry in [
        json!("-local.teem.native"),
        json!({"package":"/custom/ui/teem-progress","options":{"config":"/custom/config.toml"}}),
    ] {
        let cli = serde_json::to_vec(&json!({"plugins":[entry]})).unwrap();
        f.write("config/opencode/cli.json", &cli);
        let output = f.install();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("consolidate"));
        assert_eq!(fs::read(f.path("config/opencode/cli.json")).unwrap(), cli);
        assert!(!f.path("config/opencode/opencode.json").exists());
        assert!(!f.path("data").exists());
    }
}

#[test]
fn selected_config_and_install_destinations_never_follow_symlinks() {
    for target in ["herdr-config", "config", "data", "bin"] {
        let f = Fixture::new();
        f.write("outside/sentinel", "private user data");
        symlink(f.path("outside"), f.path(target)).unwrap();
        let output = f.install();
        assert!(!output.status.success(), "followed {target}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("symlink"), "{target}: {error}");
        assert!(
            error.contains(f.path(target).to_str().unwrap()),
            "{target}: {error}"
        );
        assert!(!f.path("config/opencode/opencode.json").exists());
        assert_eq!(
            fs::read_to_string(f.path("outside/sentinel")).unwrap(),
            "private user data"
        );
        assert_eq!(fs::read_dir(f.path("outside")).unwrap().count(), 1);
    }
}
