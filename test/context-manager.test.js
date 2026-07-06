import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { createDefaultEngine } from '../src/index.js';
import { withTempDir, writeFixture } from './helpers/tmp.js';

test('builds context from explicit and retrieved files with redaction', async () => {
  await withTempDir('context-manager', async (repo) => {
    await writeFixture(repo, 'README.md', '# Project rules\n');
    await writeFixture(repo, 'src/auth.js', 'export function refreshToken() { return "ok"; }\n');
    await writeFixture(repo, 'src/log.txt', 'token=supersecretvalue\n');

    const engine = createDefaultEngine({ config: { dataDir: path.join(repo, '.damaian') } });
    const index = await engine.indexer.indexRepository(repo);
    const context = await engine.contextManager.buildContext({
      repositoryRoot: repo,
      repositoryId: index.repositoryId,
      taskId: 'task_1',
      prompt: 'How does refresh token work?',
      index,
      explicitPaths: ['src/log.txt']
    });

    assert.equal(context.files.includes('README.md'), true);
    assert.equal(context.files.includes('src/auth.js'), true);
    assert.equal(context.items.some((item) => item.content.includes('[REDACTED_')), true);
  });
});

test('builds context from unique file names mentioned in the prompt', async () => {
  await withTempDir('context-file-mentions', async (repo) => {
    await writeFixture(repo, 'README.md', '# Project rules\n');
    await writeFixture(repo, 'docs/USER_GUIDE.md', '# User guide\n\nDesktop setup notes.\n');

    const engine = createDefaultEngine({ config: { dataDir: path.join(repo, '.damaian') } });
    const index = await engine.indexer.indexRepository(repo);
    const context = await engine.contextManager.buildContext({
      repositoryRoot: repo,
      repositoryId: index.repositoryId,
      taskId: 'task_1',
      prompt: 'Check USER_GUIDE.md for correctness against current implementation.',
      index
    });

    assert.equal(context.files.includes('docs/USER_GUIDE.md'), true);
  });
});
