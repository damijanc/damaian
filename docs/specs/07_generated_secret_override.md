# Feature Spec: Reviewable Generated-Secret Block

Status: Done
Order: 7 of 7
Related spec sections: `ai_coding_assistant_specification.md` §7.10 (Secret Detection — "Warn: tell the user generated code may contain a hardcoded secret", "Override: allow explicit user or admin override when policy permits"), §7.7 (Diff and Patch Engine).

## 1. Motivation

`Apply Selected` failed on a generated `README.md` of setup instructions with:

```
Generated content appears to contain a hardcoded secret
```

Two independent defects met there.

**The detection was wrong.** `SecretScanner::scan_credential_assignments`
(`crates/workspace-engine/src/secret_scanner.rs`) flags any line where a
credential keyword (`password`, `api_key`, `token`, …) is followed by `=` or
`:` and at least eight non-space characters. It has no notion of a
documentation placeholder, so every "set your key like this" line in a setup
doc looks exactly like a hardcoded secret. Running the scanner over this
repository's own documentation reproduced it:

```
docs/USER_GUIDE.md:137 [credential_assignment] "your-deepseek-api-key"
docs/USER_GUIDE.md:144 [credential_assignment] "your-deepseek-api-key"
```

**The block had no way out.** `PatchEngine::apply_patch` already took an
`allow_generated_secrets: bool`, but `EditOrchestrator::apply_stored_patch`
passed a hardcoded `false`, and both front ends go through it. The parameter
was unreachable from any user-facing path. The only escape was flipping
`block_generated_secrets` in config — a permanent, global policy change made to
get past a single false positive. §7.10 requires an override; it was never
wired up.

The two compound: a false positive with no override is an unrecoverable dead
end, and the pressure it creates is to disable the protection globally.

Note that generated file content reaches the check unredacted on the desktop
path. The text-envelope flow redacts the model's output before parsing it
(`edit.rs`), but the structured `propose_patch` tool call from spec #3 builds
`ProposedChange::new_content` straight from `arguments_json`
(`generated_edit_from_tool_call`, `chat.rs`), where only the assistant's prose
is redacted. So the apply-time scan is the first and only check on tool-call
generated content — which is why this path is where the bug surfaced.

## 2. Goals

- Stop flagging documentation placeholders as credentials, in warnings and in
  redaction alike — redacting `your-api-key` out of a README destroys the
  instruction and protects nothing.
- Make the block reviewable: tell the user *which* file tripped the check and
  *what* fired, before anything is written, and let them apply anyway.
- Keep the decision explicit, per-apply, and audited.

## 3. Non-Goals

- Weakening the structural detectors (private keys, AWS, JWT, GCP, Azure,
  provider token prefixes, custom patterns). None are exempted.
- A persistent "always allow" setting. `block_generated_secrets` remains the
  only durable policy switch; the override is per-apply and never remembered.
- Rewriting the scanner as a regex/entropy engine.
- Redacting tool-call arguments upstream. Considered and deliberately left
  alone: the apply-time check is the intended gate, and redacting generated
  file content before the user ever sees the diff would corrupt legitimate
  edits silently. Worth revisiting separately.

## 4. Design

### 4.1 Placeholder exemption

`is_placeholder_value` (`secret_scanner.rs`) gates the two assignment-shaped
detectors — `scan_credential_assignments` and the password field of
`scan_database_passwords`. A value is exempt when it is:

- template or variable syntax: contains `<`, `>`, `{`, `}`, `%`, or starts `$`
- an existing redaction placeholder, or a `keychain:` reference
- eight or more repeats of a single character (`xxxxxxxx`, `********`)
- an environment variable name: only `A-Z`, `0-9`, `_`, and containing `_`
  (underscore required so an all-caps credential such as an AWS key id is not
  swept up)
- self-describing: contains `your`, `placeholder`, `changeme`, `change-me`,
  `change_me`, `example`, `dummy`, `sample`, `replace`, `insert`, `todo`, or
  `redacted`

The exemption lives in `scan`, so it applies to `redact` as well. That is the
point: these values must survive into the model's context and into stored
diffs, or the documentation they belong to stops making sense.

Deliberately excluded from the word list: `test`. `sk_test_…` is a real
provider key format.

### 4.2 Preview before apply

`PatchEngine::prepare_files` was extracted from `apply_patch` — it resolves the
selection, runs the drift check, and computes the exact bytes each file would
receive, including partial-hunk reconstruction. `apply_patch` and the new
`preview_generated_secrets` both build on it, so the content the user is warned
about is byte-for-byte the content that would be written.

`preview_generated_secrets` returns `Vec<GeneratedSecretWarning>` — path,
distinct categories, count. Never the matched text, so the value cannot leak
through the UI or the audit log.

`apply_patch` now collects every flagged file before deciding, rather than
returning on the first, so one block names them all.

### 4.3 Wiring

- `EditOrchestrator::apply_stored_patch` takes `allow_generated_secrets` and
  passes it through; `preview_stored_patch_secrets` exposes the preview.
- **Desktop** `/api/apply-patch` accepts `allow_secrets=1`. Without it, and
  with the policy on, the route previews first and — if anything is flagged —
  returns `200` with `blockedBySecrets: [{path, count, categories}]` and
  `appliedFiles: []`, having written nothing. A blocked apply is data, not an
  error string: the UI cannot render a useful choice from `error.message`.
- **UI** renders the findings inline under the patch actions with `Apply
  Anyway` / `Cancel`. Accepting re-sends the identical selection with
  `allow_secrets=1`.
- **CLI** `apply-patch … [--allow-generated-secrets]`. Without the flag it
  prints each finding to stderr plus how to re-run, then applies (and the
  engine block still stops it).

### 4.4 Audit

`stored_patch_applied` gains `generatedSecretOverride=<bool>`, so an override
is always distinguishable from an ordinary apply in the trail.

## 5. Acceptance Criteria

- A generated setup README containing `export DEEPSEEK_API_KEY="your-api-key"`
  and `postgres://app:${DB_PASSWORD}@host` applies with no warning, and the
  stored diff still shows the instructions unredacted.
- A real hardcoded credential still blocks by default.
- A blocked apply writes nothing and reports path, count, and categories —
  never the value.
- Accepting applies the identical selection, and the audit log records
  `generatedSecretOverride=true`.
- The scanner reports nothing across this repository's own documentation.

## 6. Implementation Notes (as built)

All of the above shipped as designed. Verification:

- 5 new tests. `workspace-engine`: placeholder exemption across nine
  documentation forms; a real credential still found next to a placeholder;
  override applies and *still warns*; preview reports without touching disk;
  a full generated setup README applies clean with its diff unredacted.
  `desktop-shell`: an HTTP round trip against `/api/apply-patch` asserting
  warn-nothing-written, then accept-and-apply, and that the matched value never
  appears in either response.
- Manual CLI end-to-end against a scratch repo: a generated setup README
  applied with `warningCount: 0` and its instructions intact on disk.
- Scanning `README.md`, `AGENTS.md`, `SECURITY.md`, and all of `docs/` reports
  zero findings; before the change, `docs/USER_GUIDE.md` produced two.
- Test count 165 → 170; full `cargo test --workspace` green.
