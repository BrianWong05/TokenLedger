// The file-save seam: a thin port over the Rust `save_csv` command, so the
// enlarge's CSV export can be driven from a fake in tests instead of the native
// dialog. The frontend assembles the CSV (data.ts `bucketCsv`); Rust owns the
// dialog + write.
import { invoke } from '@tauri-apps/api/core';

export interface ExportPort {
  // Opens the native save dialog seeded with `filename` and writes `contents`.
  // Resolves true if a file was written, false if the user cancelled.
  saveCsv(filename: string, contents: string): Promise<boolean>;
}

export const tauriExport: ExportPort = {
  saveCsv: (filename, contents) => invoke('save_csv', { filename, contents }),
};
