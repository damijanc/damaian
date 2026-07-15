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

function providerDefaults(provider) {
  switch (provider) {
    case 'deepseek':
    case 'deedseek':
      return {
        provider: 'deepseek',
        model: process.env.DEEPSEEK_MODEL ?? 'deepseek-chat',
        baseUrl: process.env.DEEPSEEK_BASE_URL ?? 'https://api.deepseek.com',
        apiKeyEnv: 'DEEPSEEK_API_KEY'
      };
    case 'openai-compatible':
    case 'custom':
      return {
        provider: 'openai-compatible',
        model: 'configured-model',
        baseUrl: 'https://api.openai.com',
        apiKeyEnv: 'OPENAI_API_KEY'
      };
    case 'openai':
    default:
      return {
        provider: 'openai',
        model: process.env.OPENAI_MODEL ?? 'gpt-4.1',
        baseUrl: process.env.OPENAI_BASE_URL ?? 'https://api.openai.com',
        apiKeyEnv: 'OPENAI_API_KEY'
      };
  }
}

export function createDefaultConfig(overrides = {}) {
  const modelProvider = overrides.model?.provider ?? 'openai';
  const modelDefaults = providerDefaults(modelProvider);
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
      provider: modelDefaults.provider,
      model: overrides.model?.model ?? modelDefaults.model,
      baseUrl: overrides.model?.baseUrl ?? modelDefaults.baseUrl,
      apiKeyEnv: overrides.model?.apiKeyEnv ?? modelDefaults.apiKeyEnv,
      reasoningLevel: overrides.model?.reasoningLevel ?? 'default',
      timeoutMs: overrides.model?.timeoutMs ?? 60_000
    },
    secretPatterns: overrides.secretPatterns ?? []
  };
}
