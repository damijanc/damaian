import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { readFile, writeFile } from 'node:fs/promises';
import { createDefaultEngine, PatchConflictError, PolicyBlockedError } from '../src/index.js';
import { withTempDir, writeFixture } from './helpers/tmp.js';

test('creates a diff and applies approved file changes safely', async () => {
  await withTempDir('patch-engine', async (repo) => {
    await writeFixture(repo, 'src/app.js', 'export const value = 1;\n');
    const engine = createDefaultEngine({ config: { dataDir: path.join(repo, '.damaian') } });
    const patch = await engine.patchEngine.createPatch(repo, [
      { path: 'src/app.js', newContent: 'export const value = 2;\n' }
    ], { taskId: 'task_1', summary: 'Update value' });

    assert.match(patch.files[0].diff, /-export const value = 1;/);
    assert.match(patch.files[0].diff, /\+export const value = 2;/);

    const result = await engine.patchEngine.applyPatch(repo, patch, { approvedBy: 'tester' });
    assert.deepEqual(result.appliedFiles, ['src/app.js']);
    assert.equal(await readFile(path.join(repo, 'src/app.js'), 'utf8'), 'export const value = 2;\n');
  });
});

test('supports adding files in new directories', async () => {
  await withTempDir('patch-new-file', async (repo) => {
    const engine = createDefaultEngine({ config: { dataDir: path.join(repo, '.damaian') } });
    const patch = await engine.patchEngine.createPatch(repo, [
      { path: 'src/features/new-file.js', newContent: 'export const ready = true;\n' }
    ], { taskId: 'task_2', summary: 'Add feature file' });

    const result = await engine.patchEngine.applyPatch(repo, patch, { approvedBy: 'tester' });

    assert.deepEqual(result.appliedFiles, ['src/features/new-file.js']);
    assert.equal(await readFile(path.join(repo, 'src/features/new-file.js'), 'utf8'), 'export const ready = true;\n');
  });
});

test('blocks apply when target changes after patch creation', async () => {
  await withTempDir('patch-conflict', async (repo) => {
    await writeFixture(repo, 'src/app.js', 'one\n');
    const engine = createDefaultEngine({ config: { dataDir: path.join(repo, '.damaian') } });
    const patch = await engine.patchEngine.createPatch(repo, [{ path: 'src/app.js', newContent: 'two\n' }]);
    await writeFile(path.join(repo, 'src/app.js'), 'user edit\n');

    await assert.rejects(
      () => engine.patchEngine.applyPatch(repo, patch, { approvedBy: 'tester' }),
      PatchConflictError
    );
  });
});

test('blocks generated hardcoded secrets by default', async () => {
  await withTempDir('patch-secret', async (repo) => {
    await writeFixture(repo, 'src/config.js', 'export const token = "";\n');
    const engine = createDefaultEngine({ config: { dataDir: path.join(repo, '.damaian') } });
    const patch = await engine.patchEngine.createPatch(repo, [
      { path: 'src/config.js', newContent: 'export const api_key = "sk_test_12345678901234567890";\n' }
    ]);

    await assert.rejects(
      () => engine.patchEngine.applyPatch(repo, patch, { approvedBy: 'tester' }),
      PolicyBlockedError
    );
  });
});
