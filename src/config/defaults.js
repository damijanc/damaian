import os from 'node:os';
import path from 'node:path';

export const DEFAULT_IGNORE_PATTERNS = [
  '.git/',
  '.gitignore',
  'node_modules/',
  'vendor/',
  '.venv/',
  'venv/',
  'dist/',
  'build/',
  'target/',
  'coverage/',
  '.damaian/',
  '*.min.js',
  '*.map'
];

export const DEFAULT_RESTRICTED_PATTERNS = [
  '.env',
  '.env.*',
  '**/.env',
  '**/.env.*',
  '*.pem',
  '*.key',
  '*.p12',
  '*.pfx',
  'id_rsa',
  'id_dsa',
  'id_ecdsa',
  'id_ed25519',
  '**/secrets/**',
  '**/credentials/**'
];

export function defaultDataDir(appName = 'DamaianClient') {
  return path.join(os.homedir(), 'Library', 'Application Support', appName);
}

export function createDefaultConfig(overrides = {}) {
  return {
    dataDir: overrides.dataDir ?? process.env.DAMAIAN_DATA_DIR ?? defaultDataDir(),
    maxFileBytes: overrides.maxFileBytes ?? 1024 * 1024,
    maxCommandOutputBytes: overrides.maxCommandOutputBytes ?? 1024 * 1024,
    allowedRoots: overrides.allowedRoots ?? [],
    ignorePatterns: overrides.ignorePatterns ?? DEFAULT_IGNORE_PATTERNS,
    restrictedPatterns: overrides.restrictedPatterns ?? DEFAULT_RESTRICTED_PATTERNS,
    commandAllowlist: overrides.commandAllowlist ?? [],
    commandBlocklist: overrides.commandBlocklist ?? [],
    requireApprovalForFileEdits: overrides.requireApprovalForFileEdits ?? true,
    requireApprovalForRiskyCommands: overrides.requireApprovalForRiskyCommands ?? true,
    requireApprovalForAllCommands: overrides.requireApprovalForAllCommands ?? false,
    blockGeneratedSecrets: overrides.blockGeneratedSecrets ?? true,
    shell: overrides.shell ?? process.env.SHELL ?? '/bin/zsh',
    audit: {
      enabled: overrides.audit?.enabled ?? true,
      retentionDays: overrides.audit?.retentionDays ?? 90,
      debugPayloads: overrides.audit?.debugPayloads ?? false
    },
    model: {
      provider: overrides.model?.provider ?? 'openai-compatible',
      model: overrides.model?.model ?? 'configured-model',
      baseUrl: overrides.model?.baseUrl ?? 'https://api.openai.com',
      apiKeyEnv: overrides.model?.apiKeyEnv ?? 'OPENAI_API_KEY',
      timeoutMs: overrides.model?.timeoutMs ?? 60_000
    },
    secretPatterns: overrides.secretPatterns ?? []
  };
}
