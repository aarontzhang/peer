export function toPlainText(input: string): string {
  return input
    .replace(/\r\n?/g, '\n')
    .replace(/```[^\n]*\n?/g, '')
    .replace(/~~~[^\n]*\n?/g, '')
    .replace(/^ {0,3}(?:[-*_])(?:\s*[-*_]){2,}\s*$/gm, '')
    .replace(/^ {0,3}#{1,6}[ \t]+/gm, '')
    .replace(/^ {0,3}>\s?/gm, '')
    .replace(/^ {0,3}(?:[-+*]|\d+[.)])\s+/gm, '')
    .replace(/^ {0,3}\[(?: |x|X)\]\s+/gm, '')
    .replace(/!\[([^\]]*)\]\([^)]+\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/(\*\*|__)(?=\S)([^\n]*?\S)\1/g, '$2')
    .replace(/(^|[^\w])(\*|_)(?=\S)([^\n]*?\S)\2/g, '$1$3')
    .replace(/~~(?=\S)([^\n]*?\S)~~/g, '$1')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

export function firstPlainTextLine(input: string): string {
  return (
    toPlainText(input)
      .split('\n')
      .map((line) => line.trim())
      .find(Boolean) ?? ''
  );
}
