# Feature Spec: Pull-Request Creation

Status: Not started
Order: 37 of 37
Roadmap: `docs/ROADMAP/05_phase_5_delivery_workflows.md`, Phase 5, Work
Package 4 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.8 (risk classification and approval), section 7.10
(secret detection). Related implementation specs:
[`06_mcp_support.md`](06_mcp_support.md) (the transport this uses instead of a
bespoke API client),
[`17_durable_task_state_and_crash_recovery.md`](17_durable_task_state_and_crash_recovery.md)
(`unknown_external_outcome` — the central mechanism here),
[`23_verification_loop.md`](23_verification_loop.md) (the check evidence the PR
body quotes), [`31_permission_profiles.md`](31_permission_profiles.md),
[`33_mcp_management_and_deferred_discovery.md`](33_mcp_management_and_deferred_discovery.md)
(per-tool approval, and the rule that a remote read-only claim is an assertion),
[`34_repository_config_trust_boundary.md`](34_repository_config_trust_boundary.md),
[`35_commit_preparation.md`](35_commit_preparation.md),
[`36_branch_and_worktree_delivery.md`](36_branch_and_worktree_delivery.md).
See also [`SECURITY.md`](../../SECURITY.md).

## 1. Motivation

This is the first thing Damaian does that other people can see.

Everything up to here is local and recoverable. A bad patch is rewound
([spec 16](16_session_checkpoints_and_rewind.md)), a bad commit is amended, a bad
branch is deleted. A push writes to a shared remote, and a pull request notifies
reviewers, triggers CI, and appears in a feed. Neither can be undone in the sense
that matters — you can delete a PR, but the notification already went out.

Two failure modes drive the design, and both are about the boundary rather than
the feature.

**A single approval covering both push and publication is wrong.** They are
different acts with different audiences: pushing a branch is visible to whoever
looks, publishing a PR actively asks people to look. A user who approves "send
this" while thinking about the first has not agreed to the second. The roadmap
requires two approvals in order, and this spec makes them structurally distinct.

**An interrupted external write is not a failure — it is an unknown.** If the
process dies after the PR request is sent and before the response arrives, the PR
may exist. Retrying creates a duplicate; assuming failure loses the work;
assuming success reports a URL that may not exist.
[Spec 17](17_durable_task_state_and_crash_recovery.md) built
`unknown_external_outcome` for exactly this, and a PR creation is its clearest
case.

There is also a question of what to build. There is no remote API client of any
kind in the product, and the roadmap is explicit: prefer MCP
([spec 06](06_mcp_support.md)), which already has per-server configuration,
approval policy, audit, and — with
[spec 33](33_mcp_management_and_deferred_discovery.md) — per-tool control. A
bespoke GitHub client would duplicate all of that and add a credential path.

## 2. Current State

- **No remote API client exists.** No GitHub, GitLab, or Jira integration of any
  kind.
- **MCP is the available transport**, delivered by
  [spec 06](06_mcp_support.md): `mcp.rs` implements the client, `McpServerConfig`
  (`config.rs:153-175`) carries per-server `enabled` and `require_approval`, and
  `auth_token_env` (`config.rs:165-167`) holds `keychain:<account>` or an
  environment variable name — a reference, never a value.
  [Spec 33](33_mcp_management_and_deferred_discovery.md) adds per-tool enable and
  approval, and establishes that a remote tool's read-only self-description is an
  assertion with no local authority.
- **Git is read-only today**; push does not exist. `git_service.rs` is `status`,
  `diff`, `suggest_commit_message`, invoked via `-C <root>`
  (`git_service.rs:37`, `:77`, `:110`).
- **`GitService::diff` redacts** (`git_service.rs:107`), which matters here for
  the same reason as in [spec 35](35_commit_preparation.md) §5.4: what is
  displayed is not what is transmitted.
- **`unknown_external_outcome` and action markers exist** in
  [spec 17](17_durable_task_state_and_crash_recovery.md) §5.1 and §5.3, including
  the rule that nothing with an unknown outcome is ever automatically repeated.
- **Check evidence is structured.**
  [Spec 23](23_verification_loop.md) §5.7 defines a completion report whose
  passed list contains only checks that ran and exited zero, derived from
  `CommandExit` evidence.
- **Credentials have an established convention**: Keychain via
  `crates/desktop-shell/src/keychain.rs`, referenced by name in config, never
  stored as a value.

## 3. Requirements

1. **Never infer a destructive or protected target branch.** If the base branch
   is ambiguous, ask. Defaulting to `main` because it is common is inferring.
2. Push and PR creation are two approvals, not one.
3. An interrupted external write is recorded as `unknown_external_outcome` and is
   never automatically retried. Before any retry, check whether the PR already
   exists.
4. Validation evidence in the draft comes from actual check runs. An unrun check
   is never described as passing.

The flow, each step separately visible, with steps 4 and 6 separately approved:
verify repository and remote target → show commits and diff → show the exact push
action and target branch → **request push approval** → show the final PR payload
→ **request external-write approval** → create the PR → return canonical identity
and status.

Draft contents: title; summary; changes by area; validation evidence from
[spec 23](23_verification_loop.md); screenshots or artifacts the user selected;
known limitations; linked issue references.

## 4. Non-goals

- Merging a pull request, automatically or otherwise. Listed as a roadmap
  non-goal and not implemented.
- Reviewing or commenting on pull requests — Phase 5 WP5, Should-tier.
- Issue-to-code — Phase 5 WP3, Should-tier.
- A bespoke GitHub, GitLab, or Bitbucket API client. §5.2 uses MCP; if a specific
  requirement makes that impossible, the roadmap requires recording why, and §7
  is where that goes.
- Force-pushing. §5.4 refuses it outright rather than gating it.
- Creating or configuring remotes, or managing credentials. Damaian uses a remote
  and a token reference the user configured.
- CI status monitoring after creation. The PR's identity and initial status are
  returned; watching it is not this work package.
- Draft-PR-specific workflows beyond setting the draft flag if the user asks.
- Editing or updating an existing pull request.

## 5. Design

### 5.1 Push and publication are different actions with different types

Requirement 2 is enforced by the shape of the approval, not by asking twice:

```rust
/// One variant per externally visible act. No variant covers both, so a
/// granted approval cannot satisfy the other step.
pub enum ExternalWrite {
    PushBranch {
        remote: String,
        local_ref: String,
        remote_ref: String,
        commits: Vec<String>,
        /// Always false. Force is refused, not approved — §5.4.
        force: bool,
    },
    CreatePullRequest {
        remote: String,
        base: String,
        head: String,
        title: String,
        body_bytes: usize,
        draft: bool,
    },
}
```

An approval is granted for a specific `ExternalWrite` value and consumed once.
The push approval names the remote and refs; the PR approval names the base, the
head, and the title the reviewers will see. Neither is a category permission, and
there is deliberately no `Allow Always` for either — a standing permission to
push would remove the review step this whole spec exists to provide.

Ordering is enforced: `CreatePullRequest` is only offered once the push it
depends on has completed. A PR against an unpushed head would fail at the remote
anyway; refusing earlier makes the sequence in requirement 2 structural.

### 5.2 Transport: MCP, with the boundary that implies

PR creation goes through an MCP tool the user has configured and enabled —
per the roadmap's instruction to prefer MCP and
[spec 33](33_mcp_management_and_deferred_discovery.md)'s management surface.
What that buys: the token stays a reference resolved into an `Authorization`
header, per-tool approval already exists, invocation history is already audited,
and no new credential path is introduced.

What it costs, and must be handled rather than assumed away:

- **The tool is third-party code Damaian cannot verify.**
  [Spec 33](33_mcp_management_and_deferred_discovery.md) §5.5 establishes that a
  remote tool's self-description carries no local authority. So the PR payload is
  built by Damaian, shown to the user in full, and passed to the tool — Damaian
  does not ask the tool what it intends to do and does not trust a `readOnlyHint`.
- **The response is untrusted data.** A returned PR URL and number are recorded
  and displayed as *reported by the tool*, and the returned text is redacted
  through `SecretScanner` before display like any other tool output. A malicious
  or broken server returning instruction-shaped text gets no more authority than
  any other tool result.
- **Push, by contrast, is local Git.** `git -C <root> push <remote> <refspec>`
  through the existing command path, which keeps the `-C` form
  (`git_service.rs:38-43`) and reuses the command runner's timeout, truncation,
  and redaction. Push does not need MCP and should not use it: it is the one
  remote operation the user's own Git credentials already handle.

If no suitable MCP tool is configured, PR creation reports that and stops — after
a successful push, which is still useful on its own. It does not fall back to a
bespoke client.

### 5.3 The base branch is asked for, not guessed

Requirement 1, and the rule most likely to be softened for convenience.

The base is resolved only from unambiguous evidence:

| Evidence | Use |
|---|---|
| The user stated it | Use it |
| The branch was created from a local branch in this session ([spec 36](36_branch_and_worktree_delivery.md) records the start point) | Offer it, pre-selected, still confirmable |
| The remote reports a default branch **and** it is the only plausible candidate | Offer it, pre-selected |
| Anything else | **Ask.** No default |

`origin/HEAD` is *evidence*, not an answer: it is frequently stale or unset, and
"the remote says `main`" plus "this branch was cut from `develop`" is exactly the
ambiguous case requirement 1 is about. A repository whose `origin/HEAD` is unset
produces a question, not a fallback to `main`.

**Protected-branch information narrows rather than widens.** Where the remote
exposes it, a protected base is shown as protected and the PR is still creatable
(that is what PRs are for) — but a *push* whose target ref is protected is
refused before the approval card is offered, since it would be rejected remotely
and the user should not be asked to approve something that cannot work.

The head is the branch just pushed. It is never inferred either: if the current
branch has no upstream and none was created this session, the push step names the
ref it would create and asks.

### 5.4 Force push is refused, not gated

`--force` and `--force-with-lease` do not appear in any command this spec
constructs, and `ExternalWrite::PushBranch.force` is always false.

Every other dangerous operation in this phase is approval-gated; this one is
refused. The reasoning: a force push destroys commits on a shared remote that
other people may already have, and the destruction is not visible in the approval
card — "push `damaian/fix-retry` to `origin`" looks identical whether it
fast-forwards or discards three of someone else's commits. An approval the user
cannot evaluate is not consent.

A non-fast-forward push therefore fails, and the failure is reported with what to
do about it: the remote branch has diverged, here is what came in, resolve it and
push again. A user who genuinely needs a force push does it in their own
terminal, where the command says what it is.

### 5.5 The unknown-outcome window

Requirement 3, and the mechanism the whole flow is built around.

Both external writes are bracketed by
[spec 17](17_durable_task_state_and_crash_recovery.md) §5.3 action markers with
`sideEffecting: true`, and both have a real window between "request sent" and
"response received" in which the outcome is genuinely unknown.

**Push.** The start marker records remote, refs, and the local commit OID. On
recovery, resolution is a read: `git ls-remote <remote> <ref>` and compare
against the recorded OID. If it matches, the push landed. This is offered as an
inspection action, not performed automatically — but it is cheap and definitive,
so the recovery prompt can offer "check whether it landed" as the first choice.

**PR creation.** The start marker records remote, base, head, and a
Damaian-generated idempotency key. On recovery, **before any retry**, search for
an existing PR with the same head and base through the same MCP server. Three
outcomes:

| Found | Action |
|---|---|
| Exactly one matching PR | It succeeded. Adopt its identity, no retry |
| None | It did not create one. Retry is safe **and still requires a fresh approval** |
| More than one | Report all of them and stop. Do not guess |

The fresh-approval point matters: an approval was consumed by the attempt that
died, and reusing it would mean the second request went out without anyone
agreeing to it. The user is told the first attempt's outcome was unknown, that no
PR was found, and asked again.

If the search itself cannot run — the MCP server is unreachable — the task stays
`unknown_external_outcome` and offers inspection with the information needed to
check by hand. It does not retry blind. Requirement 3's "check whether the PR
already exists" is a precondition, not a best effort.

### 5.6 The PR body, and not overstating it

Requirement 4. The validation section is generated from
[spec 23](23_verification_loop.md)'s completion report, whose passed list already
contains only checks that ran and exited zero (§5.7 there, derived from
`CommandExit` evidence with `exit_code == Some(0)`).

Three rules carried through to the body:

- **Checks that did not run are listed as not run**, with the reason —
  `declined_by_user`, `no_running_target`, no detected check. Silence would read
  as "nothing to report", and a reviewer reads a PR body as a claim about what
  was verified.
- **`stale_after_partial_acceptance`** ([spec 23](23_verification_loop.md) §5.6)
  is stated. A check that passed against five files when three were accepted did
  not verify what is in the PR, and saying so is the difference between evidence
  and decoration.
- **Unverified plan steps** ([spec 21](21_task_plan_progress_and_budget.md)) go
  in known limitations. A step completed with no observable evidence is exactly
  what a reviewer should look at first.

The body is generated, shown in full, and **editable before the publication
approval**. It is the text reviewers will read and attribute to the author, so
the author sees it as it will appear.

**The body is scanned for secrets before the approval**, against the composed
text rather than a redacted preview — the same reasoning as
[spec 35](35_commit_preparation.md) §5.4. A PR body is more exposed than a
commit: it is rendered in a web UI, emailed in notifications, and indexed. Check
output quoted into the body is the realistic source, and command output has
already been redacted once by the time it reaches a `Finding`
([spec 22](22_findings_model_and_panel.md) §5.6) — so this is a second net over
a path that should already be clean, which is the right place for one.

### 5.7 Approval and policy

- Code mode only ([spec 20](20_working_modes.md)). Ask, Plan, and Review cannot
  push or publish.
- Permitted only under a profile allowing remote writes
  ([spec 31](31_permission_profiles.md)), which repository config cannot grant
  ([spec 34](34_repository_config_trust_boundary.md)). A cloned repository cannot
  enable pushing.
- The MCP server and the specific PR-creation tool must both be enabled by the
  user ([spec 33](33_mcp_management_and_deferred_discovery.md) §5.2), and the
  tool's `require_approval` cannot be cleared by repository config.
- Every step is audited: remote, refs, commit OIDs, base, head, title, body
  size, approval decisions, and outcome — never the token, and never the body
  content beyond its size.

### 5.8 Documentation

`docs/USER_GUIDE.md`: the two approvals and why they are separate, why Damaian
asks for the base branch instead of assuming, why force push is refused, and what
the PR body's validation section does and does not claim.
`docs/TROUBLESHOOTING.md`: what to do when a push is rejected as non-fast-forward,
how to resolve an `unknown_external_outcome` PR creation by hand, what to do when
the duplicate search finds more than one PR, and where external writes appear in
the audit log. `SECURITY.md`: remote writes as a boundary, alongside the existing
command, path, secret, and key boundaries.

## 6. Acceptance Criteria

- Push and PR creation each require their own approval, in that order, and a
  granted push approval cannot satisfy the PR step — asserted at the type level
  and by test.
- `CreatePullRequest` is not offered until the push it depends on has completed.
- Neither action offers `Allow Always`.
- An ambiguous base branch produces a question, not a default — asserted with a
  fixture whose `origin/HEAD` is unset, and with one where the remote default and
  the branch's start point disagree.
- A push to a protected ref is refused before an approval card is offered.
- No command constructed by this work package contains `--force` or
  `--force-with-lease`, and a non-fast-forward push fails with a diverged-branch
  explanation.
- A push interrupted mid-flight is `unknown_external_outcome`, and resolution
  compares `git ls-remote` against the recorded OID rather than re-pushing.
- A PR creation interrupted mid-flight is `unknown_external_outcome`, searches
  for an existing PR with the same head and base **before** any retry, adopts a
  single match, requires a **fresh approval** to retry when none is found, and
  stops without guessing when more than one is found.
- When the duplicate search cannot run, the task stays unknown and offers
  inspection rather than retrying.
- The PR body's validation section matches the recorded check results exactly:
  only checks that ran and exited zero appear as passing, checks that did not run
  are listed with their reason, and stale-after-partial-acceptance is stated.
- Unverified plan steps appear under known limitations.
- The body is editable before the publication approval, and the edited text is
  what is sent.
- A seeded fake secret in the composed body is detected before the approval and
  requires an explicit override.
- The tool's response is treated as untrusted: the returned identity is recorded
  as reported, and returned text is redacted before display.
- PR creation without a configured and enabled MCP tool reports that and stops,
  with no fallback client, and a successful push is not rolled back.
- Pushing and publishing are refused in Ask, Plan, and Review modes, and under a
  profile denying remote writes; repository config cannot enable either.
- No token value appears in any audit field, log, or displayed output.
- No pull request is merged by this work package.
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no increase in
  approval-policy violations.
- No test contacts a real remote — push and PR operations are exercised against a
  local bare repository and a mock MCP server.

## 7. Implementation Notes

To be completed during implementation. Record:

- Which MCP server was used for PR creation, which tool, and whether its search
  capability was sufficient for the §5.5 duplicate check. **If no server offered
  a usable search, say so prominently**: requirement 3's precondition cannot be
  met without it, and the honest outcome is that interrupted PR creations are
  resolved by hand rather than that the check was skipped.
- Whether a bespoke client proved necessary for any part, and why — the roadmap
  requires recording the reason if MCP is not used.
- How often the base-branch question fired versus being answered from evidence.
  If it asks on nearly every PR, the evidence rules in §5.3 are too strict to be
  usable and should be revisited — but by adding evidence sources, never by
  adding a default.
- The rehearsal on a throwaway remote, per the phase's validation requirement,
  before the first real PR.
