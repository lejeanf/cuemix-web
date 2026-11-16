import type { DeviceInfo } from './protocol';

/** A routing source or destination: its name and the connector group it belongs to. */
export interface IoName {
  name: string;
  group: string;
}

function groupOf(name: string): string {
  if (/^(mic|line|main|analog)/i.test(name)) return 'Analog';
  if (/^s\/?pdif/i.test(name)) return 'S/PDIF';
  if (/^adat|^optical/i.test(name)) return 'ADAT';
  if (/^phones/i.test(name)) return 'Phones';
  return '';
}

/** Routing sources: the device's physical inputs (the first mixer inputs), else "In n". */
export function routingInputs(n: number, device: DeviceInfo | null | undefined): IoName[] {
  return Array.from({ length: n }, (_, i) => {
    const name = device?.input_names[i] ?? `In ${i + 1}`;
    return { name, group: groupOf(name) };
  });
}

/** UltraLite-mk5 physical outputs, in hardware order. */
const MK5_OUTPUTS: readonly string[] = [
  'Main L',
  'Main R',
  ...Array.from({ length: 8 }, (_, i) => `Line ${i + 3}`),
  'Phones L',
  'Phones R',
  'S/PDIF L',
  'S/PDIF R',
  ...Array.from({ length: 8 }, (_, i) => `ADAT ${i + 1}`),
];

/** Routing destinations: named physical outputs on a known device, else "Out n". */
export function routingOutputs(n: number, device: DeviceInfo | null | undefined): IoName[] {
  const known = device && device.model.startsWith('UL5') && n === MK5_OUTPUTS.length;
  return Array.from({ length: n }, (_, o) => {
    const name = known ? MK5_OUTPUTS[o] : `Out ${o + 1}`;
    return { name, group: groupOf(name) };
  });
}

/** Runs of consecutive items in the same group: for group headers. */
export function groupSpans(items: readonly IoName[]): { group: string; span: number; start: number }[] {
  const out: { group: string; span: number; start: number }[] = [];
  items.forEach((it, i) => {
    const last = out[out.length - 1];
    if (last && last.group === it.group) last.span++;
    else out.push({ group: it.group, span: 1, start: i });
  });
  return out;
}
