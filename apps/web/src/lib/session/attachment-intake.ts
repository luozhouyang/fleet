/**
 * Browser-side attachment intake.
 *
 * Reading a `File` needs DOM APIs, so this lives apart from the pure composer
 * model and only runs in the browser.
 */

import { checkAttachment, type PendingAttachment } from './composer-model';

export type AttachmentIntakeResult =
  { ok: true; attachment: PendingAttachment } | { ok: false; error: string };

function readAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error('Could not read the file.'));
    reader.onload = () => {
      const result = String(reader.result ?? '');
      // A data URL looks like `data:<mime>;base64,<payload>`.
      const comma = result.indexOf(',');
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.readAsDataURL(file);
  });
}

let attachmentCounter = 0;

/**
 * Validate and read one dropped or pasted file.
 *
 * The limits mirror what the machine socket can carry comfortably; the daemon
 * receives every attachment as base64 JSON, so a large file would stall the
 * connection for every other event.
 */
export async function intakeAttachment(
  file: File,
  existing: number,
  imagesSupported: boolean,
): Promise<AttachmentIntakeResult> {
  const check = checkAttachment(file, existing, imagesSupported);
  if (!check.ok) return check;
  try {
    const dataBase64 = await readAsBase64(file);
    attachmentCounter += 1;
    return {
      ok: true,
      attachment: {
        id: `att_${Date.now().toString(36)}_${attachmentCounter}`,
        name: file.name,
        mimeType: file.type || 'application/octet-stream',
        size: file.size,
        dataBase64,
        previewUrl: check.image ? URL.createObjectURL(file) : undefined,
      },
    };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : 'Could not read the file.',
    };
  }
}

/** Release browser resources held by an attachment. */
export function releaseAttachment(attachment: PendingAttachment): void {
  if (attachment.previewUrl) URL.revokeObjectURL(attachment.previewUrl);
}
