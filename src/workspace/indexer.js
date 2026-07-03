import path from 'node:path';
import { lstat, readFile, readdir, realpath, stat } from 'node:fs/promises';
import { createId, nowIso, sha256 } from '../core/hash.js';
import { DEFAULT_IGNORE_PATTERNS } from '../config/defaults.js';
import { detectLanguage, extractImports, extractSymbols } from './language.js';
import { isIgnoredByRules, parseIgnorePatterns } from './ignore.js';

function tokenize(text) {
  return [...new Set(String(text).toLowerCase().match(/[a-z0-9_.$/-]+/g) ?? [])];
}

function looksBinary(buffer) {
  return buffer.subarray(0, 8000).includes(0);
}

function chunkText(content, maxChunkChars = 2400) {
  const chunks = [];
  let start = 0;
  while (start < content.length) {
    const end = Math.min(content.length, start + maxChunkChars);
    chunks.push({
      ordinal: chunks.length,
      start,
      end,
      text: content.slice(start, end)
    });
    start = end;
  }
  return chunks;
}

function scoreRecord(record, queryTerms) {
  const searchable = [
    record.path,
    record.language,
    ...record.symbols,
    ...record.imports,
    ...record.terms
  ].join(' ').toLowerCase();
  let score = 0;
  for (const term of queryTerms) {
    if (record.path.toLowerCase().includes(term)) score += 5;
    if (record.symbols.some((symbol) => symbol.toLowerCase().includes(term))) score += 4;
    if (searchable.includes(term)) score += 2;
  }
  return score;
}

export class RepositoryIndex {
  constructor({ repositoryId, rootPath, indexedAt, files, skipped }) {
    this.repositoryId = repositoryId;
    this.rootPath = rootPath;
    this.indexedAt = indexedAt;
    this.files = files;
    this.skipped = skipped;
  }

  keywordSearch(query, { limit = 8 } = {}) {
    const queryTerms = tokenize(query);
    return this.files
      .map((record) => ({ record, score: scoreRecord(record, queryTerms) }))
      .filter((result) => result.score > 0)
      .sort((a, b) => b.score - a.score || a.record.path.localeCompare(b.record.path))
      .slice(0, limit)
      .map(({ record, score }) => ({
        path: record.path,
        language: record.language,
        symbols: record.symbols,
        imports: record.imports,
        score,
        snippet: record.chunks[0]?.text.slice(0, 500) ?? ''
      }));
  }

  semanticSearch(query, { limit = 8 } = {}) {
    const queryTerms = tokenize(query);
    return this.files
      .flatMap((record) => record.chunks.map((chunk) => ({ record, chunk })))
      .map(({ record, chunk }) => {
        const chunkTerms = tokenize(`${record.path} ${record.symbols.join(' ')} ${chunk.text}`);
        const overlap = queryTerms.filter((term) => chunkTerms.includes(term)).length;
        return {
          path: record.path,
          language: record.language,
          symbols: record.symbols,
          score: overlap,
          snippet: chunk.text.slice(0, 500),
          chunkOrdinal: chunk.ordinal
        };
      })
      .filter((result) => result.score > 0)
      .sort((a, b) => b.score - a.score || a.path.localeCompare(b.path))
      .slice(0, limit);
  }

  toJSON() {
    return {
      repositoryId: this.repositoryId,
      rootPath: this.rootPath,
      indexedAt: this.indexedAt,
      files: this.files,
      skipped: this.skipped
    };
  }
}

export class ProjectIndexer {
  constructor({ config, scanner, auditLog } = {}) {
    this.config = config;
    this.scanner = scanner;
    this.auditLog = auditLog;
  }

  async indexRepository(rootPath) {
    const root = await realpath(rootPath);
    const files = [];
    const skipped = [];
    const defaultRules = parseIgnorePatterns(this.config.ignorePatterns ?? DEFAULT_IGNORE_PATTERNS);

    const walk = async (directory, relativeDirectory, inheritedRules) => {
      let rules = inheritedRules;
      const ignorePath = path.join(directory, '.gitignore');
      try {
        const content = await readFile(ignorePath, 'utf8');
        rules = [...inheritedRules, ...parseIgnorePatterns(content.split('\n'), relativeDirectory)];
      } catch {
        rules = inheritedRules;
      }

      const entries = await readdir(directory, { withFileTypes: true });
      for (const entry of entries) {
        const relativePath = [relativeDirectory, entry.name].filter(Boolean).join('/');
        const absolutePath = path.join(directory, entry.name);
        const isDirectory = entry.isDirectory();

        if (isIgnoredByRules(rules, relativePath, isDirectory)) {
          skipped.push({ path: relativePath, reason: 'ignored' });
          continue;
        }

        if (entry.isSymbolicLink()) {
          const resolved = await realpath(absolutePath);
          if (!resolved.startsWith(`${root}${path.sep}`)) {
            skipped.push({ path: relativePath, reason: 'symlink_outside_root' });
            continue;
          }
        }

        if (isDirectory) {
          await walk(absolutePath, relativePath, rules);
          continue;
        }

        if (!entry.isFile()) {
          skipped.push({ path: relativePath, reason: 'not_regular_file' });
          continue;
        }

        await this.addFile(root, absolutePath, relativePath, files, skipped);
      }
    };

    await walk(root, '', defaultRules);

    const index = new RepositoryIndex({
      repositoryId: createId('repo'),
      rootPath: root,
      indexedAt: nowIso(),
      files,
      skipped
    });

    await this.auditLog?.record('repository_indexed', {
      actor: 'system',
      repositoryId: index.repositoryId,
      resourcePath: root,
      status: 'complete',
      fileCount: files.length,
      skippedCount: skipped.length
    });

    return index;
  }

  async addFile(root, absolutePath, relativePath, files, skipped) {
    const fileStat = await stat(absolutePath);
    if (fileStat.size > this.config.maxFileBytes) {
      skipped.push({ path: relativePath, reason: 'too_large', size: fileStat.size });
      return;
    }

    const linkStat = await lstat(absolutePath);
    if (!linkStat.isFile()) {
      skipped.push({ path: relativePath, reason: 'not_regular_file' });
      return;
    }

    const buffer = await readFile(absolutePath);
    if (looksBinary(buffer)) {
      skipped.push({ path: relativePath, reason: 'binary' });
      return;
    }

    const content = buffer.toString('utf8');
    const secretFindings = this.scanner.scan(content);
    if (secretFindings.length > 0) {
      skipped.push({ path: relativePath, reason: 'contains_secret', findingCount: secretFindings.length });
      return;
    }

    const language = detectLanguage(relativePath);
    const symbols = extractSymbols(content, language);
    const imports = extractImports(content, language);
    files.push({
      repositoryId: root,
      path: relativePath,
      language,
      size: fileStat.size,
      modifiedTime: fileStat.mtime.toISOString(),
      contentHash: sha256(buffer),
      included: true,
      symbols,
      imports,
      terms: tokenize(`${relativePath} ${symbols.join(' ')} ${imports.join(' ')} ${content}`),
      chunks: chunkText(content).map((chunk) => ({
        ...chunk,
        textHash: sha256(chunk.text)
      }))
    });
  }
}
