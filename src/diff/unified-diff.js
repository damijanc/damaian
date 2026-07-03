function splitLines(content) {
  if (content.length === 0) return [];
  return content.split('\n');
}

export function createUnifiedDiff(oldContent, newContent, filePath) {
  if (oldContent === newContent) return '';

  const oldLines = splitLines(oldContent);
  const newLines = splitLines(newContent);
  const lines = [
    `--- a/${filePath}`,
    `+++ b/${filePath}`,
    `@@ -1,${oldLines.length} +1,${newLines.length} @@`
  ];

  for (const line of oldLines) lines.push(`-${line}`);
  for (const line of newLines) lines.push(`+${line}`);
  return `${lines.join('\n')}\n`;
}
