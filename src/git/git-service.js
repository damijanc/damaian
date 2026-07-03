import { spawn } from 'node:child_process';

function runGit(rootPath, args, { timeoutMs = 30_000 } = {}) {
  return new Promise((resolve) => {
    const child = spawn('git', ['-C', rootPath, ...args], { shell: false });
    let stdout = '';
    let stderr = '';
    const timer = setTimeout(() => child.kill('SIGTERM'), timeoutMs);
    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString();
    });
    child.on('close', (exitCode) => {
      clearTimeout(timer);
      resolve({ exitCode, stdout, stderr });
    });
  });
}

function parsePorcelain(raw) {
  return raw
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const indexStatus = line[0];
      const worktreeStatus = line[1];
      const pathText = line.slice(3);
      return {
        path: pathText.includes(' -> ') ? pathText.split(' -> ').at(-1) : pathText,
        raw: line,
        staged: indexStatus !== ' ' && indexStatus !== '?',
        worktree: worktreeStatus !== ' ',
        untracked: indexStatus === '?' && worktreeStatus === '?',
        conflicted: ['AA', 'DD', 'AU', 'UD', 'UA', 'DU', 'UU'].includes(`${indexStatus}${worktreeStatus}`)
      };
    });
}

export class GitService {
  constructor({ auditLog } = {}) {
    this.auditLog = auditLog;
  }

  async status(rootPath) {
    const result = await runGit(rootPath, ['status', '--porcelain=v1']);
    const status = {
      clean: result.exitCode === 0 && result.stdout.trim().length === 0,
      exitCode: result.exitCode,
      raw: result.stdout,
      stderr: result.stderr,
      files: result.exitCode === 0 ? parsePorcelain(result.stdout) : []
    };
    await this.auditLog?.record('git_status_read', {
      actor: 'system',
      resourcePath: rootPath,
      status: result.exitCode === 0 ? 'complete' : 'failed',
      exitCode: result.exitCode,
      fileCount: status.files.length
    });
    return status;
  }

  async diff(rootPath, { staged = false } = {}) {
    const args = staged ? ['diff', '--staged'] : ['diff'];
    const result = await runGit(rootPath, args);
    await this.auditLog?.record('git_diff_read', {
      actor: 'system',
      resourcePath: rootPath,
      status: result.exitCode === 0 ? 'complete' : 'failed',
      exitCode: result.exitCode,
      staged
    });
    if (result.exitCode !== 0) throw new Error(result.stderr || 'git diff failed');
    return result.stdout;
  }

  async recentLog(rootPath, { limit = 5 } = {}) {
    const result = await runGit(rootPath, ['log', `-${limit}`, '--oneline']);
    if (result.exitCode !== 0) throw new Error(result.stderr || 'git log failed');
    return result.stdout;
  }

  suggestCommitMessage({ summary = '', changedFiles = [] } = {}) {
    const fileHint = changedFiles.length === 1 ? ` ${changedFiles[0]}` : '';
    const normalized = summary.trim().replace(/[.!?]$/, '');
    return normalized ? `chore:${fileHint} ${normalized}`.replace(/\s+/g, ' ') : 'chore: update workspace changes';
  }
}
