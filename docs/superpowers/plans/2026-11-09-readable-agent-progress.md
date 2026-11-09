# Readable workflow and agent progress

Goal: replace visible workflow hashes with short titles and show worker, approved
check, reviewer and revision progress, plus real native agent todo lists when present.

1. Add regression tests for optional titles without changing legacy request identity;
   derive readable labels for old workflows. Keep UUIDs in machine-readable metadata.
2. Project per-round worker/check/two-reviewer/final-gate rows from recorded evidence.
   Future stages remain pending, failures remain failures, and executions are not
   represented as passing review verdicts. Preserve manual Foreman plan entries.
3. For new managed agents only, optionally create a private ignored OpenCode todo
   configuration where no project config would be overwritten. Record an immutable
   request capability flag before admission; only flagged tasks get todowrite permission.
   Existing native sessions and tracked project configuration remain unchanged.
4. Poll exact native sessions for completed todowrite calls, with bounded reads,
   persistent backoff and final collection. Render these as agent-reported detail,
   never test/review evidence. Do not infer todos from prose or fabricate tool history.
5. Run the full local suite, diff checks and two independent read-only reviews. Back
   up idle pilot state before refreshing only Teem/MCP. Do not replay model work or
   submit a paid test. Report native UI qualification separately.
