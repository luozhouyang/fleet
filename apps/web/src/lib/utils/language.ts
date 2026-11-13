const languageByExtension: Record<string, string> = {
  c: 'c',
  cc: 'cpp',
  cpp: 'cpp',
  cs: 'csharp',
  css: 'css',
  go: 'go',
  graphql: 'graphql',
  h: 'c',
  hpp: 'cpp',
  html: 'html',
  ini: 'ini',
  java: 'java',
  js: 'javascript',
  json: 'json',
  jsonc: 'json',
  jsx: 'javascript',
  kt: 'kotlin',
  less: 'less',
  lua: 'lua',
  md: 'markdown',
  mdx: 'markdown',
  php: 'php',
  proto: 'protobuf',
  py: 'python',
  rb: 'ruby',
  rs: 'rust',
  scss: 'scss',
  sh: 'shell',
  bash: 'shell',
  zsh: 'shell',
  sql: 'sql',
  svelte: 'svelte',
  swift: 'swift',
  toml: 'ini',
  ts: 'typescript',
  tsx: 'typescript',
  vue: 'html',
  xml: 'xml',
  yaml: 'yaml',
  yml: 'yaml',
};

const languageByFilename: Record<string, string> = {
  dockerfile: 'dockerfile',
  makefile: 'makefile',
  'cargo.lock': 'toml',
  'cargo.toml': 'toml',
  '.gitignore': 'plaintext',
  '.env': 'ini',
};

export function languageForPath(path: string, provided?: string): string {
  if (provided) return provided;
  const filename = path.split('/').pop()?.toLowerCase() ?? '';
  if (languageByFilename[filename]) return languageByFilename[filename];
  const extension = filename.includes('.') ? filename.split('.').pop() : undefined;
  return extension ? (languageByExtension[extension] ?? 'plaintext') : 'plaintext';
}
