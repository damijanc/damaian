import { readFile } from 'node:fs/promises';
import { AccessDeniedError } from '../core/errors.js';
import { fileHash } from '../core/hash.js';
import { PathPolicy } from './path-policy.js';

function looksBinary(buffer) {
  return buffer.subarray(0, 8000).includes(0);
}

export class FileAccessController {
  constructor({ config, auditLog, scanner, pathPolicy = new PathPolicy(config) } = {}) {
    this.config = config;
    this.auditLog = auditLog;
    this.scanner = scanner;
    this.pathPolicy = pathPolicy;
  }

  async readFile(rootPath, requestedPath, options = {}) {
    const { taskId, repositoryId, allowRestricted = false } = options;
    const target = await this.pathPolicy.resolveExisting(rootPath, requestedPath);
    await this.pathPolicy.assertNotRestricted(target.relativePath, { allowRestricted });
    const stat = await this.pathPolicy.assertRegularFile(target.absolutePath);

    if (stat.size > this.config.maxFileBytes) {
      throw new AccessDeniedError('File exceeds configured size limit', {
        relativePath: target.relativePath,
        size: stat.size,
        maxFileBytes: this.config.maxFileBytes
      });
    }

    const buffer = await readFile(target.absolutePath);
    if (looksBinary(buffer)) {
      throw new AccessDeniedError('Binary file reads are denied by default', { relativePath: target.relativePath });
    }

    const rawContent = buffer.toString('utf8');
    const redacted = this.scanner.redact(rawContent);
    const result = {
      repositoryId,
      taskId,
      path: target.relativePath,
      absolutePath: target.absolutePath,
      hash: await fileHash(target.absolutePath),
      content: redacted.text,
      redactionStatus: redacted.findings.length > 0 ? 'redacted' : 'clean',
      findings: redacted.findings.map(({ category, start, end, length, placeholder }) => ({
        category,
        start,
        end,
        length,
        placeholder
      }))
    };

    await this.auditLog?.record('file_read', {
      actor: 'assistant',
      repositoryId,
      taskId,
      resourcePath: target.relativePath,
      status: 'allowed',
      redactionStatus: result.redactionStatus,
      findingCount: result.findings.length
    });

    return result;
  }
}
