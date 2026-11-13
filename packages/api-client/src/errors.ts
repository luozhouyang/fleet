import type { ApiError } from '@agent-fleet/protocol-types';

type ErrorInit = {
  message: string;
  code: string;
  status?: number;
  details?: unknown;
  requestId?: string;
  cause?: unknown;
};

export class ApiClientError extends Error {
  readonly status?: number;
  readonly code: string;
  readonly details?: unknown;
  readonly requestId?: string;

  constructor(init: ErrorInit) {
    super(init.message, { cause: init.cause });
    this.name = 'ApiClientError';
    this.status = init.status;
    this.code = init.code;
    this.details = init.details;
    this.requestId = init.requestId;
  }
}

export function isApiClientError(error: unknown): error is ApiClientError {
  return error instanceof ApiClientError;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function looksLikeApiError(value: unknown): value is ApiError {
  return (
    isRecord(value) && typeof value.code === 'string' && typeof value.message === 'string'
  );
}

/** Accepts a bare `ApiError` or the common `{ "error": ApiError }` wrapper. */
export function extractApiError(value: unknown): ApiError | null {
  if (looksLikeApiError(value)) {
    return {
      code: value.code,
      message: value.message,
      details: value.details,
    };
  }
  if (isRecord(value) && looksLikeApiError(value.error)) {
    return {
      code: value.error.code,
      message: value.error.message,
      details: value.error.details,
    };
  }
  return null;
}

export function errorFromResponse(
  status: number,
  body: unknown,
  requestId?: string,
): ApiClientError {
  const apiError = extractApiError(body);
  return new ApiClientError({
    status,
    code: apiError?.code ?? `http_${status}`,
    message: apiError?.message ?? `Request failed with HTTP ${status}`,
    details: apiError?.details ?? body,
    requestId,
  });
}
