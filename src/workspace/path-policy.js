import path from 'node:path';
import { lstat, realpath } from 'node:fs/promises';
import { AccessDeniedError } from '../core/errors.js';
import { DEFAULT_RESTRICTED_PATTERNS } from '../config/defaults.js';
import { isIgnoredByRules, parseIgnorePatterns } from './ignore.js';

function isInside(root, target) {
  const relative = path.relative(root, target);
  return relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative));
}

async function nearestExistingAncestor(candidatePath, root) {
  let current = path.dirname(candidatePath);
  while (true) {
    try {
      return await realpath(current);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      if (current === root || current === path.dirname(current)) throw error;
      current = path.dirname(current);
    }
  }
}

export class PathPolicy {
  constructor({ allowedRoots = [], restrictedPatterns = DEFAULT_RESTRICTED_PATTERNS } = {}) {
    this.allowedRoots = allowedRoots;
    this.restrictedRules = parseIgnorePatterns(restrictedPatterns);
  }

  async canonicalRoot(rootPath) {
    const resolved = await realpath(rootPath);
    if (this.allowedRoots.length === 0) return resolved;

    const allowed = await Promise.all(this.allowedRoots.map((allowedRoot) => realpath(allowedRoot)));
    if (!allowed.some((allowedRoot) => isInside(allowedRoot, resolved))) {
      throw new AccessDeniedError('Repository root is not allowed', { rootPath });
    }
    return resolved;
  }

  async resolveExisting(rootPath, requestedPath) {
    const root = await this.canonicalRoot(rootPath);
    const absoluteCandidate = path.isAbsolute(requestedPath)
      ? requestedPath
      : path.join(root, requestedPath);
    const resolved = await realpath(absoluteCandidate);

    if (!isInside(root, resolved)) {
      throw new AccessDeniedError('Path resolves outside the selected repository', { requestedPath });
    }

    return {
      root,
      absolutePath: resolved,
      relativePath: path.relative(root, resolved).split(path.sep).join('/')
    };
  }

  async resolveForWrite(rootPath, requestedPath) {
    const root = await this.canonicalRoot(rootPath);
    const absolutePath = path.resolve(path.isAbsolute(requestedPath) ? requestedPath : path.join(root, requestedPath));

    if (!isInside(root, absolutePath)) {
      throw new AccessDeniedError('Write target is outside the selected repository', { requestedPath });
    }

    const resolvedParent = await nearestExistingAncestor(absolutePath, root);

    if (!isInside(root, resolvedParent)) {
      throw new AccessDeniedError('Write target resolves outside the selected repository', { requestedPath });
    }

    return {
      root,
      absolutePath,
      relativePath: path.relative(root, absolutePath).split(path.sep).join('/')
    };
  }

  isRestricted(relativePath, isDirectory = false) {
    return isIgnoredByRules(this.restrictedRules, relativePath, isDirectory);
  }

  async assertNotRestricted(relativePath, { isDirectory = false, allowRestricted = false } = {}) {
    if (!allowRestricted && this.isRestricted(relativePath, isDirectory)) {
      throw new AccessDeniedError('Path is restricted by policy', { relativePath });
    }
  }

  async assertRegularFile(absolutePath) {
    const stat = await lstat(absolutePath);
    if (!stat.isFile()) throw new AccessDeniedError('Path is not a regular file', { absolutePath });
    return stat;
  }
}
