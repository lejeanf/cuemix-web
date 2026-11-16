import { S } from './strings';

/**
 * The 12 output EQ slots of the device (props 1022-1026, index = slot * 3 + band).
 * They are NOT 12 mono outputs: Main and Phones are one stereo slot each.
 */
export interface EqSlot {
  /** Slot index in `state.output_eq`. */
  slot: number;
  name: string;
  /** Slot 1 does not exist on the device: never shown, never written. */
  hidden: boolean;
}

export const OUTPUT_EQ_SLOTS: readonly EqSlot[] = [
  { slot: 0, name: S.main, hidden: false },
  { slot: 1, name: '', hidden: true },
  ...[2, 3, 4, 5, 6, 7, 8, 9].map((slot) => ({ slot, name: `${S.lines} ${slot + 1}`, hidden: false })),
  { slot: 10, name: S.phones, hidden: false },
  { slot: 11, name: S.reverb, hidden: false },
];

/** Slots to show, limited to the ones the state actually has. */
export const visibleEqSlots = (count: number): EqSlot[] =>
  OUTPUT_EQ_SLOTS.filter((s) => !s.hidden && s.slot < count);

/** Display name of a slot (falls back to `Slot n` when out of the table). */
export const eqSlotName = (slot: number): string => OUTPUT_EQ_SLOTS[slot]?.name || `Slot ${slot}`;
