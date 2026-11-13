import { DiffLineType, type FileDiff } from '@agent-fleet/protocol-types';

export interface DiffSides {
  original: string;
  modified: string;
}

/**
 * Reconstructs editable text from the structured hunks so Monaco can render
 * the diff. Unchanged regions outside hunks are intentionally omitted.
 */
export function diffSides(file: FileDiff): DiffSides {
  const original: string[] = [];
  const modified: string[] = [];

  for (const hunk of file.hunks) {
    for (const line of hunk.lines) {
      switch (line.line_type) {
        case DiffLineType.Context:
          original.push(line.content);
          modified.push(line.content);
          break;
        case DiffLineType.Deleted:
          original.push(line.content);
          break;
        case DiffLineType.Added:
          modified.push(line.content);
          break;
        case DiffLineType.NoNewline:
          break;
        default:
          break;
      }
    }
  }

  return {
    original: original.join('\n'),
    modified: modified.join('\n'),
  };
}
