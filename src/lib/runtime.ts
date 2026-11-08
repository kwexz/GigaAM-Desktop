/**
 * Runtime detection for the desktop shell.
 *
 * Tauri injects `__TAURI_INTERNALS__` into the webview before app code runs, so
 * this check is true inside the desktop app and false in a plain browser.
 * Keep it as the single source of truth for "may I call desktop IPC?".
 */
export function isTauriRuntime(): boolean {
	return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
