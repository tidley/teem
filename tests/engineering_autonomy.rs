use serde_json::Value;

#[test]
fn shipped_profile_defaults_to_engineering_decisions_and_final_briefing() {
    let p: Value = serde_json::from_str(include_str!("../examples/pilot-foreman.json")).unwrap();
    let prompt = p["agents"]["teem_foreman"]["system"].as_str().unwrap();
    for rule in [
        "Make routine in-scope engineering decisions without asking for approval",
        "At completion or a genuine blocker, briefly report decisions",
        "configured target project",
        "Do not silently renew budgets",
        "read-only reference",
        "Read the actual check failures and review findings yourself",
        "explain the trigger and broken user-visible behaviour",
        "Implement clear in-scope fixes without another approval",
        "Do not ask the user what the regressions are",
        "Distinguish a proposed fix, an implemented fix and a verified fix",
    ] {
        assert!(prompt.contains(rule), "Missing guidance: {rule}");
    }
}

#[test]
fn initial_instructions_separate_authority_execution_and_reporting() {
    let p: Value = serde_json::from_str(include_str!("../examples/pilot-foreman.json")).unwrap();
    let prompt = p["agents"]["teem_foreman"]["system"].as_str().unwrap();
    for heading in [
        "## Outcome",
        "## Authority",
        "## Execution",
        "## Evidence",
        "## Reporting",
    ] {
        assert!(prompt.contains(heading), "Missing section: {heading}");
    }
    assert!(prompt.contains("Reuse explicit approval for its stated scope; a new workflow phase does not require the same approval again"));
    assert!(prompt.contains("Local commits require local-commit authority"));
    assert!(
        !prompt
            .contains("Local commits of verified authorised work are permitted in this workflow")
    );
}

#[test]
fn explicit_human_review_override_is_honoured_without_forging_qualification() {
    let p: Value = serde_json::from_str(include_str!("../examples/pilot-foreman.json")).unwrap();
    let prompt = p["agents"]["teem_foreman"]["system"].as_str().unwrap();
    for rule in [
        "Explicit human review override",
        "Do not ask Tom to repeat an already explicit waiver",
        "never passed or independently qualified",
        "local-commit-only means NO PUSH",
        "Do not commit inside a retained managed",
    ] {
        assert!(
            prompt.contains(rule),
            "Missing explicit override rule: {rule}"
        );
    }
}
