/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_API_BASE_URL?: string;
  /** Injected only by the dev build; otherwise the auth gate is shown. */
  readonly VITE_DEFAULT_API_TOKEN?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
