# Feature Spec: Docker Command Support

Status: Done
Order: 13 of 13
Related spec sections: `ai_coding_assistant_specification.md` section 7.4
(command approval), section 7.6 (tool and action orchestrator), section 7.8
(risk classification and approval), and `docs/specs/03_structured_tool_calling.md`,
`docs/specs/10_persistent_command_approval.md`.

## 1. Motivation

A real Damaian session exposed a command-support gap: the assistant needed a
Docker command result, but instead asked the user to run the command manually
because it was unable or unwilling to request it through Damaian.

Docker is common in local development workflows. Agents need it for tasks such
as checking container status, starting a project service, running integration
tests, inspecting logs, or reproducing a bug in the same environment the user
uses.

Docker is also not a normal validation command. A single command can pull from
the network, build images, start background processes, mount host paths, expose
ports, mutate named volumes, remove containers, or run with elevated container
capabilities. Supporting Docker therefore belongs in the existing command
approval boundary, not in the automatic sandbox-safe path.

## 2. Current State

- `run_command` is available to native-tool-capable providers, but its tool
  description says it runs a "read-only local shell command" and lists only
  read-only examples (`crates/workspace-engine/src/chat.rs`). This nudges the
  model away from requesting approved side-effecting commands, even though the
  policy layer can already create approval cards for them.
- `CommandPolicy` has no Docker-specific classification
  (`crates/workspace-engine/src/command_policy.rs`). A `docker ...` command
  falls through to the unknown high-risk bucket, requiring approval but showing
  generic "unknown effects" messaging.
- `CommandRunner` executes approved commands through the configured shell with
  `-lc` (`crates/workspace-engine/src/command_runner.rs`). In a macOS GUI
  launch, that environment may not include the same `PATH` as the user's
  interactive Terminal, so Docker can be installed but unavailable to Damaian as
  `docker`.
- The existing approval UI already supports one-time approval and exact-command
  `Allow Always` (`docs/specs/10_persistent_command_approval.md`). Docker
  should reuse that mechanism instead of adding a parallel approval system.

## 3. Requirements

1. The assistant can request Docker commands through Damaian when a Docker
   command is the right way to inspect or validate the selected repository.
2. Docker commands are never classified as sandbox-safe automatic commands by
   default. They require explicit approval unless the exact command is already
   allowlisted and `require_approval_for_all_commands` is false.
3. Docker commands receive a Docker-specific high-risk classification instead
   of the generic unknown-command classification.
4. The approval prompt explains Docker-specific effects: container lifecycle
   changes, image and volume mutation, host path mounts, port exposure, and
   possible network access.
5. The model-facing command tool description makes the true contract clear:
   the model may request local commands, and Damaian will either run safe
   read-only commands automatically or pause for user approval.
6. Common Docker failures are diagnosed clearly and fed back to the model:
   executable not found, Docker Desktop or the Docker daemon not running,
   permission denied, and unavailable Compose support.
7. All Docker command output continues through the existing truncation, secret
   redaction, session persistence, and audit logging paths.
8. `Allow Always` remains exact-command only. Allowing `docker ps` must not
   allow `docker compose up`, and allowing `docker compose up -d` must not allow
   `docker compose down -v`.
9. Admin and repository policy controls remain authoritative. Existing
   `command_blocklist`, `require_approval_for_all_commands`, and shell-control
   checks must not be bypassed for Docker.
10. The implementation remains macOS-only and does not introduce Node.js or a
    Docker SDK into the shipped application.

## 4. Non-goals

- Building a Docker management UI.
- Adding a Docker API client, Compose parser, or container orchestration layer.
- Automatically starting Docker Desktop or installing Docker.
- Treating Docker commands as read-only by subcommand. Even `docker ps` talks to
  a privileged local daemon, so phase 1 keeps the rule simple: Docker is
  approval-gated.
- Broad allowlist patterns such as `docker *` or `docker compose *`.
- Bypassing the user's shell, credentials, Docker context, or local daemon
  configuration.
- Guaranteeing that Docker commands are safe inside the container. Damaian
  gates host command execution; Docker's own runtime isolation remains Docker's
  responsibility.

## 5. Design

### 5.1 Command Classification

Add a Docker-specific branch to `CommandPolicy::classify_pattern` before the
unknown-command fallback:

```rust
if is_docker_command(&normalized) {
    return CommandClassification {
        command: normalized,
        risk: CommandRisk::High,
        blocked: false,
        requires_approval: true,
        reasons: vec![
            "Docker command may start containers, mount host paths, mutate images or volumes, expose ports, or use the network".to_string(),
        ],
        expected_effects: "Potential Docker daemon, workspace, network, or background-service effects".to_string(),
        may_use_network: true,
    };
}
```

`is_docker_command` should match:

- `docker`
- `docker ...`
- `docker-compose`
- `docker-compose ...`

The branch does not weaken the existing hard blocks. `is_blocked_command`,
`command_blocklist`, and `contains_shell_control` continue to run first. A
repository or admin policy can still block Docker entirely with a prefix entry
such as:

```text
command_blocklist=docker
```

### 5.2 Tool and Prompt Wording

Update the native `run_command` tool description so it does not falsely imply
that the model may request only read-only commands.

The tool description should say:

```text
Request a local shell command in the selected repository. Damaian runs
sandbox-safe read-only commands automatically and pauses for user approval
before running commands with side effects, network access, shell control, or
unknown risk.
```

The fallback `DAMAIAN_COMMAND_V1` system prompt should keep recommending
read-only commands when they are sufficient, but explicitly state that the model
may request approval-gated commands when the user's task requires them.

This is a behavior clarification, not a policy change. The orchestrator remains
the authority that decides whether a command runs, pauses, or is blocked.

### 5.3 Docker Availability Diagnostics

When an approved Docker command exits before doing useful work, Damaian should
append a concise diagnostic to the command result before it is fed back to the
model. The raw stderr is still preserved and redacted; the diagnostic is an
additional interpretation layer.

Recognized cases:

- `docker: command not found`, `command not found: docker`, or exit 127:
  explain that the GUI-launched app may not have the user's Terminal `PATH`.
  Suggest an absolute executable path or launching the app from a shell for
  development.
- `Cannot connect to the Docker daemon`, `Is the docker daemon running`, or
  Docker Desktop socket failures: explain that Docker Desktop or the daemon is
  not running or not reachable from Damaian.
- permission-denied messages involving the Docker socket: explain that the
  current user or environment cannot access the daemon.
- `docker compose` plugin not found: suggest trying `docker-compose` if that is
  installed, or installing/enabling the Compose plugin.

The diagnostic text must not expose unredacted command output. It should be
included in the same tool result that the model already receives after command
execution, so the model can adapt in the resumed turn.

### 5.4 macOS Executable Resolution

Do not add a general shell-environment editor in this feature. For phase 1, the
support path is diagnostic-first.

If implementation proves that diagnostics are not enough, add a narrow,
audited Docker executable resolution step:

- Only applies when the command begins with `docker` or `docker-compose`.
- Checks known macOS CLI locations such as `/usr/local/bin`,
  `/opt/homebrew/bin`, and `/Applications/Docker.app/Contents/Resources/bin`.
- Records the resolved executable path in the audit log.
- Preserves the user-visible command string and approval prompt exactly as
  requested.

This follow-up must not execute arbitrary discovery commands and must not
silently broaden `PATH` for unrelated commands.

### 5.5 Approval and Persistence

Docker reuses the existing approval machinery:

- First request creates an approval card.
- `Approve Run` runs that exact command once.
- `Allow Always`, when eligible under `allow_always_eligible`, writes the exact
  command to the selected repository's `.damaian/config.conf`.
- Shell-control commands, blocked commands, and
  `require_approval_for_all_commands=true` do not offer `Allow Always`.

No Docker-specific permanent permission is introduced.

### 5.6 Documentation

Update the User Guide's command section to mention Docker as an example of an
approval-gated local development command.

Update Troubleshooting with a short Docker entry covering:

- `docker` missing from the GUI app environment.
- Docker Desktop or the daemon not running.
- Compose plugin versus legacy `docker-compose`.
- Where to look in the audit log for the proposed command and exit status.

## 6. Acceptance Criteria

- Asking Damaian to inspect containers can produce a `run_command` request for
  `docker ps` rather than a prose instruction asking the user to run it
  manually.
- `docker ps` classifies as `high`, `requiresApproval: true`, and includes a
  Docker-specific reason and expected effects.
- `docker compose up -d` also requires approval, even when
  `require_approval_for_risky_commands=false`.
- A blocked policy such as `command_blocklist=docker` prevents a Docker command
  from running and reports the normal policy-blocked error.
- Approving a Docker proposal executes through the existing command runner,
  stores stdout and stderr refs, redacts secrets, and feeds the result back to
  the model when the command was part of a chat turn.
- Choosing `Allow Always` for `docker ps` allowlists only `docker ps`; a later
  `docker ps -a` or `docker compose ps` still prompts.
- If Docker is not on Damaian's `PATH`, the command result includes a clear
  diagnostic about the macOS GUI launch environment instead of leaving only a
  bare shell error.
- If the Docker daemon is not reachable, the command result says that Docker
  Desktop or the daemon is not running or reachable.
- The feature adds no runtime Node.js dependency and no Docker SDK dependency.
- Unit or integration tests cover Docker classification, blocklist precedence,
  exact-command allowlist behavior, tool-description wording, and at least the
  command-not-found diagnostic.

## 7. Implementation Notes

Landed in the initial implementation:

- `CommandPolicy` now classifies `docker`, `docker ...`, `docker-compose`, and
  `docker-compose ...` as high-risk, approval-required commands with
  Docker-specific reasons and expected effects. Shell-control checks,
  hard-blocked commands, configured blocklists, and exact-command allowlists
  still run before the Docker branch.
- `run_command` tool wording and the `DAMAIAN_COMMAND_V1` fallback prompt now
  describe the real command contract: read-only commands can run automatically,
  while side-effecting, networked, Docker, shell-control, or unknown-risk
  commands pause for approval.
- `CommandRunner` appends redacted Docker diagnostics for missing CLI, daemon
  reachability, socket permission, and Compose-plugin failures. The diagnostic
  is stored with stderr and therefore reaches saved command output and resumed
  chat tool context through the existing flow.
- `docs/USER_GUIDE.md` and `docs/TROUBLESHOOTING.md` document Docker approval
  behavior and common macOS environment/daemon failures.
- Tests cover Docker classification, blocklist precedence, exact-command
  allowlisting, native tool wording, and Docker command diagnostics.
