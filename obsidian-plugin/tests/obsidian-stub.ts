// Test-only stand-in for the Obsidian API. The real `obsidian` package ships
// type declarations with no runtime entry, so it cannot be imported under
// Vitest; vitest.config.ts aliases it to this stub.

export class Plugin {}
export class Notice {}
export class App {}

export function normalizePath(path: string): string {
  return path;
}

export function requestUrl(): Promise<never> {
  throw new Error("requestUrl is not available in tests");
}
