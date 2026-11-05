type TauriInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

declare global {
  interface Window {
    __TAURI__?: {
      core?: { invoke: TauriInvoke };
    };
    __TAURI_INTERNALS__?: {
      invoke: TauriInvoke;
    };
  }
}

type NativeRequest = { action: string; [key: string]: unknown };

function tauriInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke;

  if (!invoke) {
    return Promise.reject(
      new Error("Layrs Desktop commands are available only inside the Tauri desktop runtime.")
    );
  }

  return invoke<T>(command, args);
}

export function selectFolder(initialDirectory?: string) {
  return tauriInvoke<string | null>("select_folder", {
    initialDirectory: initialDirectory || undefined
  });
}

export function nativeWorkspace<T>(root: string, request: NativeRequest): Promise<T> {
  return tauriInvoke<T>("native_workspace", { root, request });
}

export function nativeGitBridge<T>(root: string, request: NativeRequest): Promise<T> {
  return tauriInvoke<T>("native_git_bridge", { root, request });
}

export function nativeCheckoutKind(root: string): Promise<"complete" | "projection" | "absent"> {
  return tauriInvoke("native_checkout_kind", { root });
}

export function nativeProjection<T>(root: string, request: NativeRequest): Promise<T> {
  return tauriInvoke<T>("native_projection", { root, request });
}

export type GitExportMode =
  | { kind: "history" }
  | { kind: "selected_history"; references: string[] }
  | { kind: "snapshot"; stepId: string };

export type GitRequest =
  | { action: "previewImport"; source: string; references: string[] }
  | { action: "applyImport"; previewId: string }
  | { action: "previewExport"; destination: string; mode: GitExportMode }
  | { action: "applyExport"; previewId: string }
  | { action: "discardPreview"; previewId: string };

export function nativeGit<T>(root: string, request: GitRequest): Promise<T> {
  return tauriInvoke<T>("native_git", { root, request });
}

export function nativeRemote<T>(
  root: string,
  request: { origin: string; caPem: string | null; expectedPrincipalId?: string; action: NativeRequest }
): Promise<T> {
  return tauriInvoke<T>("native_remote", { root, request });
}
