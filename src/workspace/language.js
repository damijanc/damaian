import path from 'node:path';

const EXTENSION_LANGUAGES = new Map([
  ['.js', 'javascript'],
  ['.jsx', 'javascript'],
  ['.mjs', 'javascript'],
  ['.cjs', 'javascript'],
  ['.ts', 'typescript'],
  ['.tsx', 'typescript'],
  ['.py', 'python'],
  ['.go', 'go'],
  ['.rs', 'rust'],
  ['.java', 'java'],
  ['.kt', 'kotlin'],
  ['.php', 'php'],
  ['.rb', 'ruby'],
  ['.cs', 'csharp'],
  ['.swift', 'swift'],
  ['.md', 'markdown'],
  ['.json', 'json'],
  ['.yml', 'yaml'],
  ['.yaml', 'yaml'],
  ['.toml', 'toml'],
  ['.xml', 'xml'],
  ['.html', 'html'],
  ['.css', 'css']
]);

function unique(values) {
  return [...new Set(values.filter(Boolean))];
}

export function detectLanguage(filePath) {
  return EXTENSION_LANGUAGES.get(path.extname(filePath).toLowerCase()) ?? 'text';
}

export function extractImports(content, language) {
  const imports = [];
  const lines = content.split('\n');
  for (const line of lines) {
    let match;
    if (['javascript', 'typescript'].includes(language)) {
      match = line.match(/^\s*import\s+.*?\s+from\s+['"]([^'"]+)['"]/)
        ?? line.match(/^\s*import\s+['"]([^'"]+)['"]/)
        ?? line.match(/require\(['"]([^'"]+)['"]\)/);
    } else if (language === 'python') {
      match = line.match(/^\s*(?:from\s+([\w.]+)\s+import|import\s+([\w.]+))/);
    } else if (language === 'go') {
      match = line.match(/^\s*"([^"]+)"/) ?? line.match(/^\s*import\s+"([^"]+)"/);
    } else if (language === 'rust') {
      match = line.match(/^\s*use\s+([^;]+);/);
    } else if (['java', 'kotlin'].includes(language)) {
      match = line.match(/^\s*import\s+([^;]+);/);
    } else if (language === 'php') {
      match = line.match(/^\s*use\s+([^;]+);/);
    }
    if (match) imports.push(match[1] ?? match[2]);
  }
  return unique(imports);
}

export function extractSymbols(content, language) {
  const symbols = [];
  const patterns = {
    javascript: [
      /\b(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/g,
      /\b(?:export\s+)?class\s+([A-Za-z_$][\w$]*)/g,
      /\b(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=/g
    ],
    typescript: [
      /\b(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/g,
      /\b(?:export\s+)?class\s+([A-Za-z_$][\w$]*)/g,
      /\b(?:export\s+)?interface\s+([A-Za-z_$][\w$]*)/g,
      /\b(?:export\s+)?type\s+([A-Za-z_$][\w$]*)\s*=/g,
      /\b(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=/g
    ],
    python: [/^\s*(?:def|class)\s+([A-Za-z_]\w*)/gm],
    go: [/\bfunc\s+(?:\([^)]+\)\s*)?([A-Za-z_]\w*)\s*\(/g, /\btype\s+([A-Za-z_]\w*)\s+(?:struct|interface)/g],
    rust: [/\b(?:pub\s+)?fn\s+([A-Za-z_]\w*)\s*\(/g, /\b(?:pub\s+)?(?:struct|enum|trait)\s+([A-Za-z_]\w*)/g],
    java: [/\b(?:class|interface|enum)\s+([A-Za-z_]\w*)/g],
    kotlin: [/\b(?:class|interface|object|fun)\s+([A-Za-z_]\w*)/g],
    php: [/\b(?:class|function|interface|trait)\s+([A-Za-z_]\w*)/g]
  };

  for (const pattern of patterns[language] ?? []) {
    let match;
    pattern.lastIndex = 0;
    while ((match = pattern.exec(content)) !== null) symbols.push(match[1]);
  }
  return unique(symbols);
}
