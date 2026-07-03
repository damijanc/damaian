import path from 'node:path';
import { mkdir, readFile, rename, unlink, writeFile } from 'node:fs/promises';
import { createId, fileHash, nowIso, sha256 } from '../core/hash.js';
import { AccessDeniedError, PatchConflictError, PolicyBlockedError } from '../core/errors.js';
import { PathPolicy } from '../workspace/path-policy.js';
import { createUnifiedDiff } from '../diff/unified-diff.js';

async function readExisting(pathToRead) {
  try {
    return await readFile(pathToRead, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

export class PatchEngine {
  constructor({ config, auditLog, scanner, pathPolicy = new PathPolicy(config) } = {}) {
    this.config = config;
    this.auditLog = auditLog;
    this.scanner = scanner;
    this.pathPolicy = pathPolicy;
  }

  async createPatch(rootPath, changes, { taskId, summary = 'Proposed workspace changes' } = {}) {
    const files = [];
    for (const change of changes) {
      const target = await this.pathPolicy.resolveForWrite(rootPath, change.path);
      await this.pathPolicy.assertNotRestricted(target.relativePath, { allowRestricted: change.allowRestricted });
      const oldContent = await readExisting(target.absolutePath);
      const newContent = change.newContent ?? '';
      const status = oldContent === null ? 'added' : change.status ?? 'modified';
      files.push({
        path: target.relativePath,
        status,
        baseHash: oldContent === null ? null : sha256(oldContent),
        newContent,
        newHash: sha256(newContent),
        diff: createUnifiedDiff(oldContent ?? '', newContent, target.relativePath)
      });
    }

    const patch = {
      id: createId('patch'),
      taskId,
      summary,
      status: 'pending',
      createdAt: nowIso(),
      files
    };

    await this.auditLog?.record('patch_proposed', {
      actor: 'assistant',
      taskId,
      patchId: patch.id,
      status: 'pending',
      files: files.map((file) => file.path),
      summary
    });

    return patch;
  }

  async applyPatch(rootPath, patch, options = {}) {
    const {
      taskId = patch.taskId,
      approvedBy = 'local_user',
      approvedPaths = patch.files.map((file) => file.path),
      allowGeneratedSecrets = false
    } = options;

    const selected = patch.files.filter((file) => approvedPaths.includes(file.path));
    if (selected.length === 0) return { patchId: patch.id, appliedFiles: [], warnings: [] };

    const prepared = [];
    const warnings = [];

    for (const file of selected) {
      const target = await this.pathPolicy.resolveForWrite(rootPath, file.path);
      await this.pathPolicy.assertNotRestricted(target.relativePath, { allowRestricted: file.allowRestricted });
      const currentContent = await readExisting(target.absolutePath);
      const currentHash = currentContent === null ? null : sha256(currentContent);
      if (currentHash !== file.baseHash) {
        throw new PatchConflictError('Target file changed after patch generation', {
          path: file.path,
          expectedHash: file.baseHash,
          actualHash: currentHash
        });
      }

      const findings = this.scanner.scan(file.newContent ?? '');
      if (findings.length > 0) {
        warnings.push({ path: file.path, category: 'generated_secret', findingCount: findings.length });
        if (this.config.blockGeneratedSecrets && !allowGeneratedSecrets) {
          throw new PolicyBlockedError('Generated content appears to contain a hardcoded secret', {
            path: file.path,
            findingCount: findings.length
          });
        }
      }

      prepared.push({ file, target, currentContent });
    }

    const rollbackDir = path.join(this.config.dataDir, 'rollback', patch.id);
    await mkdir(rollbackDir, { recursive: true });

    for (const item of prepared) {
      const rollbackPath = path.join(rollbackDir, item.file.path.replaceAll('/', '__'));
      await writeFile(
        rollbackPath,
        JSON.stringify(
          {
            path: item.file.path,
            baseHash: item.file.baseHash,
            capturedAt: nowIso(),
            content: item.currentContent
          },
          null,
          2
        ),
        'utf8'
      );

      await mkdir(path.dirname(item.target.absolutePath), { recursive: true });
      if (item.file.status === 'deleted') {
        if (item.currentContent === null) throw new AccessDeniedError('Cannot delete a file that does not exist', { path: item.file.path });
        await unlink(item.target.absolutePath);
      } else {
        const tempPath = `${item.target.absolutePath}.damaian-${process.pid}-${Date.now()}.tmp`;
        await writeFile(tempPath, item.file.newContent, 'utf8');
        await rename(tempPath, item.target.absolutePath);
      }

      await this.auditLog?.record('file_modified', {
        actor: 'system',
        taskId,
        patchId: patch.id,
        approvedBy,
        resourcePath: item.file.path,
        status: item.file.status,
        baseHash: item.file.baseHash,
        newHash: item.file.status === 'deleted' ? null : await fileHash(item.target.absolutePath),
        rollbackPath
      });
    }

    await this.auditLog?.record('patch_applied', {
      actor: 'system',
      taskId,
      patchId: patch.id,
      approvedBy,
      status: 'applied',
      files: prepared.map((item) => item.file.path),
      warningCount: warnings.length
    });

    return {
      patchId: patch.id,
      appliedFiles: prepared.map((item) => item.file.path),
      warnings
    };
  }
}
