import test from 'node:test';
import assert from 'node:assert/strict';
import { createDefaultConfig, CommandPolicy } from '../src/index.js';

test('classifies read-only commands as low risk', () => {
  const policy = new CommandPolicy({ config: createDefaultConfig({ dataDir: '/tmp/damaian-test' }) });
  const result = policy.classify('git status --short');
  const showResult = policy.classify('git show --stat');

  assert.equal(result.risk, 'low');
  assert.equal(result.blocked, false);
  assert.equal(showResult.risk, 'low');
  assert.equal(showResult.blocked, false);
});

test('requires approval for validation commands', () => {
  const policy = new CommandPolicy({ config: createDefaultConfig({ dataDir: '/tmp/damaian-test' }) });
  const result = policy.classify('npm test');

  assert.equal(result.risk, 'medium');
  assert.equal(result.requiresApproval, true);
});

test('blocks destructive commands', () => {
  const policy = new CommandPolicy({ config: createDefaultConfig({ dataDir: '/tmp/damaian-test' }) });
  const result = policy.classify('rm -rf .');

  assert.equal(result.risk, 'blocked');
  assert.equal(result.blocked, true);
});

test('requires approval for shell control syntax', () => {
  const policy = new CommandPolicy({ config: createDefaultConfig({ dataDir: '/tmp/damaian-test' }) });
  const result = policy.classify('ls | head');

  assert.equal(result.risk, 'high');
  assert.equal(result.requiresApproval, true);
});
