/**
 * Tauri command wrappers.
 *
 * Task 3.3 — wraps `pick_file` and `read_file` from the Tauri backend so that
 * the frontend never imports from `@tauri-apps/api` directly.
 */

import { invoke } from '@tauri-apps/api/core';

/**
 * Open a native file-picker dialog and return the chosen path, or null if the
 * user cancelled.
 */
export const pickFile = (): Promise<string | null> =>
  invoke<string | null>('pick_file');

/**
 * Read the file at `path` and return its raw bytes as a JS number array.
 * Tauri serialises `Vec<u8>` to a JSON array of numbers.
 * Convert to a Uint8Array before handing to WASM:
 * ```ts
 * const numArr = await readFile(path);
 * const bytes = new Uint8Array(numArr);
 * ```
 */
export const readFile = (path: string): Promise<number[]> =>
  invoke<number[]>('read_file', { path });

/**
 * Open a native save-file dialog and write `contents` to the chosen path.
 * Returns the saved path, or null if the user cancelled the dialog.
 *
 * The `contents` Uint8Array is serialised as a JSON number array for the
 * Tauri bridge (Tauri deserialises it as `Vec<u8>` on the Rust side).
 */
export const saveFile = (
  defaultName: string,
  contents: Uint8Array,
): Promise<string | null> =>
  invoke<string | null>('save_file', {
    defaultName,
    contents: Array.from(contents),
  });

/**
 * Load the persisted preferences JSON string from the app config dir.
 * Returns '{}' if no prefs file exists yet.
 */
export const loadPrefs = (): Promise<string> =>
  invoke<string>('load_prefs');

/**
 * Persist a JSON string to the app config dir as prefs.json.
 */
export const savePrefs = (contents: string): Promise<void> =>
  invoke<void>('save_prefs', { contents });
