import { createDefaultConfig } from '../config/defaults.js';
import { AuditLog } from '../audit/audit-log.js';
import { SecretScanner } from '../security/secret-scanner.js';
import { PathPolicy } from '../workspace/path-policy.js';
import { FileAccessController } from '../workspace/file-access.js';
import { ProjectIndexer } from '../workspace/indexer.js';
import { ContextManager } from '../context/context-manager.js';
import { CommandPolicy } from '../commands/command-policy.js';
import { CommandRunner } from '../commands/command-runner.js';
import { GitService } from '../git/git-service.js';
import { PatchEngine } from '../patch/patch-engine.js';
import { OpenAICompatibleAdapter } from '../model/openai-compatible-adapter.js';

export class WorkspaceEngine {
  constructor({ config, scanner, auditLog, pathPolicy, fileAccess, indexer, contextManager, commandPolicy, commandRunner, git, patchEngine, modelAdapter }) {
    this.config = config;
    this.scanner = scanner;
    this.auditLog = auditLog;
    this.pathPolicy = pathPolicy;
    this.fileAccess = fileAccess;
    this.indexer = indexer;
    this.contextManager = contextManager;
    this.commandPolicy = commandPolicy;
    this.commandRunner = commandRunner;
    this.git = git;
    this.patchEngine = patchEngine;
    this.modelAdapter = modelAdapter;
  }
}

export function createDefaultEngine(overrides = {}) {
  const config = createDefaultConfig(overrides.config ?? {});
  const scanner = overrides.scanner ?? new SecretScanner({ customPatterns: config.secretPatterns });
  const auditLog = overrides.auditLog ?? new AuditLog({
    dataDir: config.dataDir,
    enabled: config.audit.enabled,
    scanner
  });
  const pathPolicy = overrides.pathPolicy ?? new PathPolicy({
    allowedRoots: config.allowedRoots,
    restrictedPatterns: config.restrictedPatterns
  });
  const fileAccess = overrides.fileAccess ?? new FileAccessController({ config, auditLog, scanner, pathPolicy });
  const indexer = overrides.indexer ?? new ProjectIndexer({ config, scanner, auditLog });
  const contextManager = overrides.contextManager ?? new ContextManager({ fileAccess, scanner });
  const commandPolicy = overrides.commandPolicy ?? new CommandPolicy({ config });
  const commandRunner = overrides.commandRunner ?? new CommandRunner({ config, commandPolicy, auditLog, scanner });
  const git = overrides.git ?? new GitService({ auditLog });
  const patchEngine = overrides.patchEngine ?? new PatchEngine({ config, auditLog, scanner, pathPolicy });
  const modelAdapter = overrides.modelAdapter ?? new OpenAICompatibleAdapter({
    baseUrl: config.model.baseUrl,
    apiKey: process.env[config.model.apiKeyEnv],
    model: config.model.model,
    timeoutMs: config.model.timeoutMs
  });

  return new WorkspaceEngine({
    config,
    scanner,
    auditLog,
    pathPolicy,
    fileAccess,
    indexer,
    contextManager,
    commandPolicy,
    commandRunner,
    git,
    patchEngine,
    modelAdapter
  });
}
