import { spawn } from 'node:child_process';
import { ApprovalRequiredError, PolicyBlockedError } from '../core/errors.js';
import { createId, nowIso } from '../core/hash.js';

function summarizeEnvironment(env, scanner) {
  const sensitiveName = /(key|token|secret|password|credential|auth)/i;
  return Object.fromEntries(
    Object.entries(env)
      .filter(([key]) => ['PATH', 'HOME', 'SHELL', 'USER', 'NODE_ENV'].includes(key) || sensitiveName.test(key))
      .map(([key, value]) => [key, sensitiveName.test(key) ? '[REDACTED_ENV]' : scanner.redact(String(value)).text])
  );
}

export class CommandRunner {
  constructor({ config, commandPolicy, auditLog, scanner } = {}) {
    this.config = config;
    this.commandPolicy = commandPolicy;
    this.auditLog = auditLog;
    this.scanner = scanner;
  }

  async run(command, { cwd, reason = '', approved = false, approvedBy = 'local_user', taskId, timeoutMs = 120_000 } = {}) {
    const runId = createId('cmd');
    const classification = this.commandPolicy.classify(command);

    await this.auditLog?.record('command_proposed', {
      actor: 'assistant',
      taskId,
      command: classification.command,
      workingDirectory: cwd,
      risk: classification.risk,
      reason,
      requiresApproval: classification.requiresApproval,
      blocked: classification.blocked
    });

    if (classification.blocked) {
      throw new PolicyBlockedError('Command is blocked by policy', { command, classification });
    }

    if (classification.requiresApproval && !approved) {
      throw new ApprovalRequiredError('Command requires user approval before execution', { command, classification });
    }

    const startedAt = nowIso();
    const startedMs = Date.now();
    const result = await new Promise((resolve) => {
      const child = spawn(command, {
        cwd,
        shell: this.config.shell,
        env: process.env
      });
      let stdout = '';
      let stderr = '';
      const appendLimited = (current, chunk) => (current + chunk.toString()).slice(-this.config.maxCommandOutputBytes);
      const timer = setTimeout(() => child.kill('SIGTERM'), timeoutMs);

      child.stdout.on('data', (chunk) => {
        stdout = appendLimited(stdout, chunk);
      });
      child.stderr.on('data', (chunk) => {
        stderr = appendLimited(stderr, chunk);
      });
      child.on('close', (exitCode, signal) => {
        clearTimeout(timer);
        const completedAt = nowIso();
        resolve({
          id: runId,
          taskId,
          command,
          workingDirectory: cwd,
          risk: classification.risk,
          approvedBy: classification.requiresApproval ? approvedBy : null,
          startedAt,
          completedAt,
          durationMs: Date.now() - startedMs,
          exitCode,
          signal,
          stdout: this.scanner.redact(stdout).text,
          stderr: this.scanner.redact(stderr).text,
          environment: summarizeEnvironment(process.env, this.scanner)
        });
      });
    });

    await this.auditLog?.record('command_executed', {
      actor: 'command',
      taskId,
      command,
      workingDirectory: cwd,
      risk: classification.risk,
      approvedBy: result.approvedBy,
      startedAt: result.startedAt,
      completedAt: result.completedAt,
      durationMs: result.durationMs,
      exitCode: result.exitCode,
      stdoutSummary: result.stdout.slice(0, 2000),
      stderrSummary: result.stderr.slice(0, 2000)
    });

    return result;
  }
}
