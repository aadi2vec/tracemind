// Dev-only invoke shim.
//
// In the compiled Tauri app, `window.__TAURI_INTERNALS__` is present and we
// forward to the real IPC bridge exactly like `@tauri-apps/api/core` does.
// In the Vite browser preview (no Tauri shell), we fall back to a small local
// HTTP server (`demo-bridge.mjs`) that shells to the real `tracemind` CLI, so
// the UI is always talking to the actual ingestion pipeline — never mocks.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";

const BRIDGE_URL = "http://localhost:3131/rpc";

function hasTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function bridgeInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const res = await fetch(BRIDGE_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ cmd, args: args ?? {} }),
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`bridge ${cmd} → ${res.status}: ${body}`);
  }
  return (await res.json()) as T;
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (hasTauri()) return tauriInvoke<T>(cmd, args);
  return bridgeInvoke<T>(cmd, args);
}
