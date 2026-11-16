use serde_json::Value;

#[test]
fn canonical_profile_only_renames_the_retained_prototype() {
    let old: Value = serde_json::from_str(include_str!("../examples/pilot-foreman.json")).unwrap();
    let current: Value = serde_json::from_str(include_str!("../examples/pilot-teem.json")).unwrap();
    let mut expected = old["agents"]["teem_foreman"].clone();
    expected["description"] = serde_json::json!(
        "Teem: deliver authorised tasks through managed workers, checks and independent reviews"
    );
    expected["system"] = serde_json::json!(
        expected["system"]
            .as_str()
            .unwrap()
            .replace("teem_foreman_", "teem_")
            .replace("foreman_", "teem_")
            .replace("You are the Teem Foreman", "You are Teem")
            .replace("the Foreman", "Teem")
    );
    assert_eq!(current["agents"]["teem"], expected);
    assert_eq!(current["default_agent"], "teem");
    for key in ["$schema", "warming", "experimental"] {
        assert_eq!(current[key], old[key]);
    }
    assert!(!current.to_string().to_lowercase().contains("foreman"));
}

#[test]
fn raw_catalog_is_canonical_without_display_time_rewriting() {
    let raw: Value = serde_json::from_str(include_str!("../src/tools.json")).unwrap();
    assert_eq!(teem::plugin::tools(), raw);
    for tool in raw.as_array().unwrap() {
        assert!(tool["name"].as_str().unwrap().starts_with("teem_"));
        assert!(
            !tool["description"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("foreman")
        );
    }
}

#[test]
fn current_docs_do_not_reintroduce_old_branding_or_tool_examples() {
    let docs = [
        include_str!("../README.md"),
        include_str!("../docs/HERDR_PLUGIN.md"),
        include_str!("../docs/TEEM_ACCESS.md"),
        include_str!("../docs/PILOT.md"),
        include_str!("../docs/ISSUE1_ENABLEMENT.md"),
        include_str!("../docs/ISSUE3_COMPACT_DISPATCH.md"),
        include_str!("../docs/ISSUE4_SIDEBAR.md"),
        include_str!("../docs/ISSUE7_STATUS_TABLES.md"),
        include_str!("../docs/ISSUE19_GLOBAL_INSTALL.md"),
        include_str!("../docs/ISSUE20_RECOVERY.md"),
        include_str!("../docs/TRUSTED_WORKERS.md"),
    ];
    for doc in docs {
        assert!(!doc.contains("Foreman"));
        for tool in teem::plugin::tools().as_array().unwrap() {
            let suffix = tool["name"]
                .as_str()
                .unwrap()
                .strip_prefix("teem_")
                .unwrap();
            assert!(!doc.contains(&format!("foreman_{suffix}")));
        }
    }
    assert!(include_str!("../docs/FOREMAN_ACCESS.md").contains("TEEM_ACCESS.md"));
}
