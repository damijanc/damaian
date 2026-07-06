const DEFAULT_PROJECT_RULES = [
  'AGENTS.md',
  'README.md',
  'CONTRIBUTING.md',
  '.editorconfig',
  'package.json',
  'pyproject.toml',
  'Cargo.toml',
  'go.mod',
  'pom.xml',
  'build.gradle'
];

function estimateTokens(text) {
  return Math.ceil(String(text).length / 4);
}

function promptPathCandidates(prompt) {
  return String(prompt)
    .split(/\s+/)
    .map((part) =>
      part
        .trim()
        .replace(/^[`"'()[\]{}<>,:;]+|[`"'()[\]{}<>,:;]+$/g, '')
        .replace(/[.!?]+$/g, '')
        .replaceAll('\\', '/')
    )
    .filter((candidate) => {
      if (
        !candidate ||
        candidate.startsWith('/') ||
        candidate.startsWith('http://') ||
        candidate.startsWith('https://') ||
        candidate.includes('../') ||
        candidate === '..' ||
        candidate.endsWith('/')
      ) {
        return false;
      }
      const name = candidate.split('/').at(-1) ?? '';
      return candidate.includes('/') || name.includes('.');
    });
}

function promptFileMentions(prompt, index) {
  if (!index?.files) return [];

  const exactPaths = new Map();
  const basenameMatches = new Map();
  for (const file of index.files) {
    exactPaths.set(file.path.toLowerCase(), file.path);
    const basename = file.path.split('/').at(-1).toLowerCase();
    basenameMatches.set(basename, [...(basenameMatches.get(basename) ?? []), file.path]);
  }

  const mentioned = [];
  const seen = new Set();
  for (const candidate of promptPathCandidates(prompt)) {
    const lower = candidate.toLowerCase();
    const basename = lower.split('/').at(-1);
    const matches = basenameMatches.get(basename) ?? [];
    const resolved = exactPaths.get(lower) ?? (!candidate.includes('/') && matches.length === 1 ? matches[0] : undefined);
    if (resolved && !seen.has(resolved)) {
      seen.add(resolved);
      mentioned.push(resolved);
    }
  }
  return mentioned;
}

export class ContextManager {
  constructor({ fileAccess, scanner } = {}) {
    this.fileAccess = fileAccess;
    this.scanner = scanner;
  }

  async buildContext({
    repositoryRoot,
    repositoryId,
    taskId,
    prompt,
    index,
    explicitPaths = [],
    conversationSummary = '',
    commandOutput = '',
    gitDiff = '',
    tokenBudget = 16_000
  }) {
    const items = [];
    const usedPaths = new Set();
    let usedTokens = 0;

    const addText = (kind, content, metadata = {}) => {
      if (!content) return false;
      const redacted = this.scanner.redact(String(content));
      const tokens = estimateTokens(redacted.text);
      if (usedTokens + tokens > tokenBudget) return false;
      usedTokens += tokens;
      items.push({ kind, content: redacted.text, tokens, redactionStatus: redacted.findings.length > 0 ? 'redacted' : 'clean', ...metadata });
      return true;
    };

    const addFile = async (filePath, kind = 'file') => {
      if (usedPaths.has(filePath)) return false;
      try {
        const file = await this.fileAccess.readFile(repositoryRoot, filePath, { taskId, repositoryId });
        const added = addText(kind, file.content, { path: file.path, hash: file.hash });
        if (added) usedPaths.add(file.path);
        return added;
      } catch {
        return false;
      }
    };

    addText('user_prompt', prompt);
    addText('session_summary', conversationSummary);

    const requestedPaths = [...explicitPaths];
    for (const path of promptFileMentions(prompt, index)) {
      if (!requestedPaths.includes(path)) requestedPaths.push(path);
    }

    for (const explicitPath of requestedPaths) {
      await addFile(explicitPath, 'explicit_file');
    }

    for (const rulePath of DEFAULT_PROJECT_RULES) {
      await addFile(rulePath, 'project_rule');
    }

    const searchResults = [
      ...(index?.keywordSearch(prompt, { limit: 8 }) ?? []),
      ...(index?.semanticSearch(prompt, { limit: 8 }) ?? [])
    ];

    for (const result of searchResults) {
      await addFile(result.path, 'retrieved_file');
    }

    addText('git_diff', gitDiff);
    addText('command_output', commandOutput);

    await this.fileAccess.auditLog?.record('context_built', {
      actor: 'system',
      repositoryId,
      taskId,
      status: 'complete',
      tokenEstimate: usedTokens,
      files: [...usedPaths]
    });

    return {
      repositoryId,
      taskId,
      tokenEstimate: usedTokens,
      items,
      files: [...usedPaths]
    };
  }
}
