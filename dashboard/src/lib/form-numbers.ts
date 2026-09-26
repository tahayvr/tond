// Helpers for `<input type="number" bind:value>` fields. Svelte binds those as
// `number | null` (null when empty), never as strings — so never call string
// methods like .trim() on them. Values are also accepted as strings so a field
// that was pre-filled with text keeps working.

export type NumberFieldValue = number | string | null | undefined;

/** A whole number from an optional number field, or null when it is empty or invalid. */
export function optionalInt(value: NumberFieldValue): number | null {
	if (value === null || value === undefined) return null;
	const text = typeof value === 'number' ? String(value) : value.trim();
	if (text === '') return null;
	const parsed = Number(text);
	return Number.isFinite(parsed) ? Math.trunc(parsed) : null;
}

/** A whole number from a number field, or `fallback` when it is empty or invalid. */
export function intOr(value: NumberFieldValue, fallback: number): number {
	return optionalInt(value) ?? fallback;
}
