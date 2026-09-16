# Feature Spec: Provider Fallback Consent

Status: Not started
Order: 56 of 56
Plan: none. Split out of
[`48_provider_limits_and_backpressure`](48_provider_limits_and_backpressure/proposal.md)
during its implementation, the way
[`07_generated_secret_override.md`](07_generated_secret_override.md) split out of
a gap analysis: the half that was recognisably its own feature rather than a
fragment of the spec it came from.
Depends on: [#48](48_provider_limits_and_backpressure/proposal.md) (the refusal
classification, the `failureKind` outcome, and the fail-closed default this
offer turns into a choice) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`10_persistent_command_approval.md`](10_persistent_command_approval.md) (the
approval-shaped decision this reuses, and whose "always" is exactly what this
must not have), [`45_crash_recovery_prompt.md`](45_crash_recovery_prompt.md)
(the card that names a refused task's outcome),
[`21_task_plan_progress_and_budget/proposal.md`](21_task_plan_progress_and_budget/proposal.md)
(the plan that must survive a switch), and
[`48_provider_limits_and_backpressure/proposal.md`](48_provider_limits_and_backpressure/proposal.md)
§5.6, of which this spec is the full statement.

## 1. Motivation

Spec 48 made a refusal fail closed: a provider that refuses a call — rate
limited, overloaded, or out of quota — fails the task, even when the user has
configured a second provider that would have answered. Fail closed is the right
default, but it is only a default. Left there, the user whose strong model is
busy or whose account is dry has no path back into their work except editing
the configuration by hand and re-asking.

The fix is not automatic fallback — that silently changes the quality of every
future answer, which is why spec 48 §5.6 rules it out. It is a consent gate: a
one-shot, refused-by-default offer to answer this turn with a different
provider, so the user is back in control without ever having been switched
behind their back.

## 2. Current State

- **Spec 48 ships the fail-closed half.** A terminal refusal returns
  `ClientError::Provider`, the chat orchestrator finishes the action marker
  `"refused"`, books a measured zero, and fails the task with a
  `failureKind` (`provider_rate_limited` / `provider_overloaded` /
  `provider_quota_exhausted` / …). The task's plan survives the failure. There
  is no switch, and `no_fallback_happens_without_approval` pins that.
- **The model adapter is constructed outside the engine.** The desktop shell
  and the CLI resolve the API key from the macOS Keychain (or an environment
  variable) and build a `CurlModelTransport` + `OpenAICompatibleAdapter`, which
  they pass into `ChatOrchestrator::ask_with_session`. The orchestrator holds no
  credentials and cannot build a fallback adapter itself. A switch therefore has
  to be orchestrated by the caller that owns the Keychain, not by the engine.
- **The pending-approval machinery already exists.** Command and patch approval
  pause a turn by persisting a `PendingChatTurn`, setting the task to
  `WaitingForApproval`, and resuming through a decision method that takes a
  `&mut dyn ModelAdapter` — the seam a fallback resume would need, since the
  resume call site can hand over a *different* adapter.
- **The config already names the candidates.** `Config.model_providers` holds
  every configured provider with its own `api_key_env`; `Config.model_provider`
  names the active one. "A second provider is available" is therefore a config
  question, not new state.

## 3. Requirements

1. On a terminal refusal, and only where a configured provider *other than the
   active one* has its own credentials, the user is offered a one-shot switch
   naming both providers. The offer is refused by default.
2. A refusal that exhausted its retry budget and a permanent refusal (quota,
   auth, bad request) both reach the offer; the offer is never made for a
   refusal that is still being retried.
3. The offer is approval-shaped: it names the model that refused and the model
   that would answer, and nothing is switched without an explicit,
   per-occurrence decision. There is no "always", and the decision is not
   remembered.
4. On approval, the turn re-issues the model call through the fallback provider
   and continues; the plan is carried across unchanged.
5. On refusal (the default), the task fails exactly as spec 48 §5.4 leaves it —
   `failureKind`, measured zero, plan intact.
6. A switch is recorded on the task and visible in the transcript, so a turn
   whose second half was answered by a different model says so.
7. Quota exhaustion is offered a switch only to a provider with *its own*
   credentials — not to a second model on the exhausted provider, which would
   hit the same empty account.

## 4. Non-goals

- **Automatic or remembered fallback.** Model routing as a deliberate cost
  strategy is Phase 3 WP7; this spec is a per-occurrence consent gate.
- **A queue or scheduler of retries.** Backpressure stays spec 48 §5.3.
- **In-engine credential handling.** The engine never sees a raw key; the
  Keychain stays the shell's responsibility, which is why the switch is
  orchestrated at the caller boundary.
- **Fallback for tool calls, patches, or commands.** This covers the model call
  only, as spec 48 §4 scopes it.

## 5. Design

### 5.1 The seam

The engine cannot build the fallback adapter, so the offer is a pause-and-resume
rather than an in-engine switch. When `run_agentic_turn` meets a terminal
refusal and a fallback candidate exists, it does what command approval does:
persist the pending turn, record a pending decision naming both providers, set
the task to `WaitingForApproval`, and return a distinct result — *not* the
`Err(ClientError::Provider)` that fail-closed returns today, and not a failed
task.

The pending decision is a new kind alongside command and patch. It carries the
refusal, the refused provider, and the fallback candidate. The decision methods
mirror the command-approval pair: an `approve` that re-issues the turn through
the fallback adapter (passed in by the caller), and a `reject` that fails the
task exactly as spec 48 §5.4 does.

### 5.2 When the offer is made

A terminal refusal only. Transient refusals that spec 48 §5.3 retries never
reach the offer mid-retry; only the exhausted or permanent outcome does. A
fallback candidate is a provider in `Config.model_providers` whose id differs
from the active provider and whose `api_key_env` is non-empty. For quota
exhaustion the candidate must be a *different* provider (requirement 7); for
rate limiting and overload a second model on the same provider is acceptable,
though a different provider is preferred.

### 5.3 The offer is one-shot and refused by default

The decision is not remembered and has no "always" arm — spec 48 §5.6's
reasoning, restated: `Allow Always` is for risks that are known and repeatable,
and "answer with a different model whenever the good one is busy" is not that.
A rejected offer fails the task; the user may re-ask and be offered again next
turn, which is the correct behaviour for a decision that depends on the
moment.

### 5.4 Recording the switch

Approval appends a `provider_fallback` event carrying both provider ids, and the
transcript shows a marker on the turn where the switch happened. Rejection
records nothing beyond spec 48's existing failure path.

### 5.5 Failure modes

- **No fallback candidate configured** → behave exactly as spec 48 does today
  (fail closed, no offer). The offer is additive; the fail-closed path it
  interrupts is never removed.
- **The fallback provider also refuses** → the turn fails with that refusal;
  the offer is not made a second time in the same turn, because a loop of
  offers is the failure mode this spec exists to avoid.
- **A crash between the offer and the decision** → the pending decision must
  reattach on restart the way a pending approval does (spec 17 §5.7), so a
  turn is never left silently waiting.

## 6. Acceptance Criteria

- A terminal refusal with no fallback configured fails the task — the existing
  spec 48 behaviour, asserted to be unchanged.
- A terminal refusal with a fallback configured returns a pending decision and
  sets the task `WaitingForApproval`, naming both providers.
- No switch happens without an explicit approval; a rejected offer fails the
  task with the refusal's `failureKind`.
- An approved offer re-issues the turn through the fallback adapter and records
  the switch on the task and in the transcript.
- A quota-exhausted offer names only a *different* provider, never a second
  model on the exhausted one.
- The offer is made once per turn: a fallback that also refuses fails the turn
  rather than offering again.
- A pending fallback decision reattaches on restart.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation.
