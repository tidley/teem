# TEEM repository and pilot setup

## Scope

Create a local TEEM repository from the original local prototype. Preserve that source checkout.
Use `local.teem` as the plugin ID, `TEEM` as its display name, and `teem.service` as its optional service.
Create a separate disposable Python pilot with two worker and two reviewer checkouts.
Keep the supervisor disabled. Do not submit prompts, approve permissions, publish, or modify existing agent conversations.

## Steps

1. Copy only the prototype source, tests, manifest, example configuration, and plugin guide.
2. Record source hashes. Initialise Git and keep machine-local `AGENTS.md` files excluded.
3. Test the TEEM identity change before adapting the package.
4. Add a small pilot project, baseline tests, and a bounded first assignment.
5. Prepare a private disabled configuration and project-local OpenCode Foreman profile.
6. Verify the code and request two independent read-only reviews before local baseline commits.
7. Create separate local pilot clones, with sparse workers and complete reviewers.
8. Link the reviewed TEEM plugin. Preserve the existing Codr plugin and its configuration.
9. If the existing runtime is healthy, create an empty pilot Foreman session without submitting a message.
10. Bind the new session, prepare project-local MCP configuration, and verify tool discovery without model execution.

No remote repository, paid pilot, automatic merge, or boot-enabled service is part of this setup.
The first model-backed assignment needs explicit model and execution approval.
