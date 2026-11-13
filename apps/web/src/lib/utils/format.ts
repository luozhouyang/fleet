import { isApiClientError } from '@agent-fleet/api-client';

export function formatBytes(bytes: number | undefined): string {
  if (bytes === undefined) return '';
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** exponent;
  const digits = exponent === 0 || value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${units[exponent]}`;
}

function toDate(value: string | undefined): Date | null {
  if (!value) return null;
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

export function formatDateTime(value: string | undefined): string {
  const date = toDate(value);
  if (!date) return '—';
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(date);
}

export function formatTime(value: string | undefined): string {
  const date = toDate(value);
  if (!date) return '';
  return new Intl.DateTimeFormat(undefined, {
    hour: '2-digit',
    minute: '2-digit',
  }).format(date);
}

export function formatRelativeTime(value: string | undefined): string {
  const date = toDate(value);
  if (!date) return 'Never';
  const seconds = Math.round((date.getTime() - Date.now()) / 1000);
  const abs = Math.abs(seconds);
  const formatter = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });

  if (abs < 45) return formatter.format(seconds, 'second');
  if (abs < 60 * 45) return formatter.format(Math.round(seconds / 60), 'minute');
  if (abs < 60 * 60 * 22) return formatter.format(Math.round(seconds / 3600), 'hour');
  if (abs < 60 * 60 * 24 * 26) {
    return formatter.format(Math.round(seconds / 86_400), 'day');
  }
  if (abs < 60 * 60 * 24 * 365) {
    return formatter.format(Math.round(seconds / (86_400 * 30)), 'month');
  }
  return formatter.format(Math.round(seconds / (86_400 * 365)), 'year');
}

export function shortId(value: string, length = 8): string {
  if (value.length <= length + 3) return value;
  return `${value.slice(0, length)}…`;
}

export function pluralize(count: number, singular: string, plural = `${singular}s`): string {
  return `${count} ${count === 1 ? singular : plural}`;
}

export function formatJson(value: unknown): string {
  if (value === undefined) return 'null';
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export function getErrorMessage(error: unknown): string {
  if (isApiClientError(error)) {
    return error.message || error.code;
  }
  if (error instanceof Error) return error.message;
  return typeof error === 'string' ? error : 'Something went wrong.';
}
