import { readFile } from 'node:fs/promises';
import path from 'node:path';

const LOW_RISK_COMMANDS = [
  /^pwd$/,
  /^ls(?:\s+[-A-Za-z0-9_./]+)*$/,
  /^git\s+status(?:\s+[-A-Za-z0-9_./=]+)*$/,
  /^git\s+diff(?:\s+[-A-Za-z0-9_./=]+)*$/,
  /^git\s+log(?:\s+[-A-Za-z0-9_./=]+)*$/
];

const VALIDATION_COMMANDS = [
  /^npm\s+test(?:\s|$)/,
  /^npm\s+run\s+(?:test|lint|typecheck|build|format)(?:\s|$)/,
  /^pytest(?:\s|$)/,
  /^python(?:3)?\s+-m\s+pytest(?:\s|$)/,
  /^mvn\s+test(?:\s|$)/,
  /^gradle\s+test(?:\s|$)/,
  /^go\s+test\s+\.\.\.(?:\s|$)/,
  /^cargo\s+test(?:\s|$)/
];

const HIGH_RISK_PATTERNS = [
  /\bnpm\s+(?:install|i|add|update)\b/,
  /\byarn\s+(?:add|install|upgrade)\b/,
  /\bpnpm\s+(?:add|install|update)\b/,
  /\bpip(?:3)?\s+install\b/,
  /\bcurl\b/,
  /\bwget\b/,
  /\bchmod\b/,
  /\bchown\b/,
  /\bgit\s+(?:commit|push|pull|reset|checkout|switch|merge|rebase|branch)\b/,
  /\bsh\s+[^ ]+/,
  /\bbash\s+[^ ]+/,
  /\bzsh\s+[^ ]+/
];

const BLOCKED_PATTERNS = [
  /\brm\s+-rf\s+(?:\/|\*|\.|\.\/|~|["']?\.["']?)(?:\s|$)/,
  /\bgit\s+reset\s+--hard\b/,
  /\bgit\s+clean\s+-fd/,
  /\bdd\s+if=/,
  /\bmkfs\b/,
  /\bshutdown\b/,
  /\breboot\b/,
  /curl\b[\s\S]*\|\s*(?:sh|bash|zsh)\b/,
  /wget\b[\s\S]*\|\s*(?:sh|bash|zsh)\b/
];

function matchesAny(patterns, command) {
  return patterns.some((pattern) => pattern.test(command));
}

function configuredPatternMatches(patterns, command) {
  return patterns.some((pattern) => {
    if (pattern instanceof RegExp) return pattern.test(command);
    return String(command).startsWith(String(pattern));
  });
}

async function fileExists(filePath) {
  try {
    await readFile(filePath);
    return true;
  } catch {
    return false;
  }
}

export class CommandPolicy {
  constructor({ config } = {}) {
    this.config = config;
  }

  classify(command) {
    const normalized = command.trim();
    const reasons = [];

    if (configuredPatternMatches(this.config.commandBlocklist, normalized) || matchesAny(BLOCKED_PATTERNS, normalized)) {
      return {
        command: normalized,
        risk: 'blocked',
        blocked: true,
        requiresApproval: true,
        reasons: ['Command matches a blocked destructive pattern'],
        expectedEffects: 'Blocked by local policy',
        mayUseNetwork: /\b(?:curl|wget|npm|pnpm|yarn|pip|git\s+(?:pull|push|fetch|clone))\b/.test(normalized)
      };
    }

    if (configuredPatternMatches(this.config.commandAllowlist, normalized)) {
      return {
        command: normalized,
        risk: 'low',
        blocked: false,
        requiresApproval: this.config.requireApprovalForAllCommands,
        reasons: ['Command matches configured allowlist'],
        expectedEffects: 'Configured safe command',
        mayUseNetwork: false
      };
    }

    if (/[;&|`<>]|\$\(/.test(normalized)) {
      return {
        command: normalized,
        risk: 'high',
        blocked: false,
        requiresApproval: true,
        reasons: ['Command contains shell control syntax and needs explicit review'],
        expectedEffects: 'Potential chained or redirected command effects',
        mayUseNetwork: /\b(?:curl|wget|npm|pnpm|yarn|pip|git\s+(?:pull|push|fetch|clone))\b/.test(normalized)
      };
    }

    if (matchesAny(LOW_RISK_COMMANDS, normalized)) {
      return {
        command: normalized,
        risk: 'low',
        blocked: false,
        requiresApproval: this.config.requireApprovalForAllCommands,
        reasons: ['Read-only command'],
        expectedEffects: 'Reads workspace or Git metadata',
        mayUseNetwork: false
      };
    }

    if (matchesAny(VALIDATION_COMMANDS, normalized)) {
      return {
        command: normalized,
        risk: 'medium',
        blocked: false,
        requiresApproval: this.config.requireApprovalForAllCommands || this.config.requireApprovalForRiskyCommands,
        reasons: ['Validation command may write build, cache, or coverage artifacts'],
        expectedEffects: 'Runs project validation and may create local artifacts',
        mayUseNetwork: false
      };
    }

    if (matchesAny(HIGH_RISK_PATTERNS, normalized)) {
      reasons.push('Command may modify dependencies, Git state, permissions, network, or shell state');
      return {
        command: normalized,
        risk: 'high',
        blocked: false,
        requiresApproval: true,
        reasons,
        expectedEffects: 'Potential workspace or external side effects',
        mayUseNetwork: /\b(?:curl|wget|npm|pnpm|yarn|pip|git\s+(?:pull|push|fetch|clone))\b/.test(normalized)
      };
    }

    return {
      command: normalized,
      risk: 'high',
      blocked: false,
      requiresApproval: true,
      reasons: ['Unknown command effects'],
      expectedEffects: 'Unknown effects until reviewed',
      mayUseNetwork: /\b(?:curl|wget|npm|pnpm|yarn|pip|git)\b/.test(normalized)
    };
  }

  async detectProjectCommands(rootPath) {
    const commands = [];
    const packagePath = path.join(rootPath, 'package.json');
    if (await fileExists(packagePath)) {
      const pkg = JSON.parse(await readFile(packagePath, 'utf8'));
      for (const name of ['test', 'lint', 'typecheck', 'build', 'format']) {
        if (pkg.scripts?.[name]) commands.push({ name, command: `npm run ${name}`, risk: this.classify(`npm run ${name}`).risk });
      }
      if (pkg.scripts?.test) commands.push({ name: 'test-shortcut', command: 'npm test', risk: this.classify('npm test').risk });
    }

    const candidates = [
      ['pyproject.toml', 'pytest'],
      ['pytest.ini', 'pytest'],
      ['pom.xml', 'mvn test'],
      ['build.gradle', 'gradle test'],
      ['go.mod', 'go test ./...'],
      ['Cargo.toml', 'cargo test']
    ];
    for (const [fileName, command] of candidates) {
      if (await fileExists(path.join(rootPath, fileName))) {
        commands.push({ name: fileName, command, risk: this.classify(command).risk });
      }
    }
    return commands;
  }
}
