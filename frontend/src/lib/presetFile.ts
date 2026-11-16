import { PRESET_NAME_RE } from './protocol';

/** Preset file format written by this version (matches the server's PRESET_SCHEMA_VERSION). */
export const PRESET_SCHEMA_VERSION = 2;

/** A valid preset name from an uploaded file name ("My mix (2).json" → "My mix 2"). */
export function presetNameFromFile(fileName: string): string {
  const base = fileName.replace(/\.json$/i, '');
  const clean = base
    .replace(/[^A-Za-z0-9 _.-]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 64)
    .trim();
  return PRESET_NAME_RE.test(clean) ? clean : 'imported';
}

/** "cuemix-<name>-2026-11-15.json" */
export function exportFileName(name: string, when: Date): string {
  const safe = presetNameFromFile(name).replace(/ /g, '_');
  return `cuemix-${safe}-${when.toISOString().slice(0, 10)}.json`;
}
