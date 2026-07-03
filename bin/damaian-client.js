#!/usr/bin/env node

import { createDefaultEngine } from '../src/index.js';

function usage() {
  return `Usage:
  damaian-client index <repo>
  damaian-client search <repo> <query>
  damaian-client read <repo> <path>
  damaian-client git-status <repo>
  damaian-client git-diff <repo>
  damaian-client detect-commands <repo>
  damaian-client classify-command <command>
`;
}

async function main(argv) {
  const [command, ...args] = argv;
  const engine = createDefaultEngine();

  if (!command || command === '--help' || command === '-h') {
    process.stdout.write(usage());
    return;
  }

  if (command === 'index') {
    const [repo] = args;
    if (!repo) throw new Error('Missing <repo>');
    const index = await engine.indexer.indexRepository(repo);
    process.stdout.write(JSON.stringify(index.toJSON(), null, 2));
    process.stdout.write('\n');
    return;
  }

  if (command === 'search') {
    const [repo, ...queryParts] = args;
    if (!repo || queryParts.length === 0) throw new Error('Missing <repo> or <query>');
    const index = await engine.indexer.indexRepository(repo);
    const results = index.keywordSearch(queryParts.join(' '), { limit: 10 });
    process.stdout.write(JSON.stringify(results, null, 2));
    process.stdout.write('\n');
    return;
  }

  if (command === 'read') {
    const [repo, requestPath] = args;
    if (!repo || !requestPath) throw new Error('Missing <repo> or <path>');
    const result = await engine.fileAccess.readFile(repo, requestPath, {
      taskId: 'cli',
      repositoryId: repo
    });
    process.stdout.write(result.content);
    if (!result.content.endsWith('\n')) process.stdout.write('\n');
    return;
  }

  if (command === 'git-status') {
    const [repo] = args;
    if (!repo) throw new Error('Missing <repo>');
    const status = await engine.git.status(repo);
    process.stdout.write(JSON.stringify(status, null, 2));
    process.stdout.write('\n');
    return;
  }

  if (command === 'git-diff') {
    const [repo] = args;
    if (!repo) throw new Error('Missing <repo>');
    const diff = await engine.git.diff(repo);
    process.stdout.write(diff);
    return;
  }

  if (command === 'detect-commands') {
    const [repo] = args;
    if (!repo) throw new Error('Missing <repo>');
    const commands = await engine.commandPolicy.detectProjectCommands(repo);
    process.stdout.write(JSON.stringify(commands, null, 2));
    process.stdout.write('\n');
    return;
  }

  if (command === 'classify-command') {
    if (args.length === 0) throw new Error('Missing <command>');
    const result = engine.commandPolicy.classify(args.join(' '));
    process.stdout.write(JSON.stringify(result, null, 2));
    process.stdout.write('\n');
    return;
  }

  throw new Error(`Unknown command: ${command}\n\n${usage()}`);
}

main(process.argv.slice(2)).catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});
