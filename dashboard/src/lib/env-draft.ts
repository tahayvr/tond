// env-draft.ts — edit write-only env vars without knowing their values.
//
// The server lists env keys with masked values and accepts patches
// (`env_set` / `env_unset`). A draft remembers which keys exist, which get a
// new value and which are removed, so a form can edit some variables and
// leave the others untouched.

import type { EnvVarEntry } from '$lib/api';

export interface EnvDraft {
	/** Keys saved on the server (values unknown) */
	saved: EnvVarEntry[];
	/** New or replaced values */
	set: Record<string, string>;
	/** Saved keys to remove */
	unset: string[];
}

export interface EnvDraftRow {
	key: string;
	/** "new" = not saved yet, "changed" = gets a new value, null = unchanged */
	change: 'new' | 'changed' | null;
	/** Whether the saved/new value is empty */
	empty: boolean;
}

export function envDraftFrom(saved: EnvVarEntry[]): EnvDraft {
	return { saved: [...saved], set: {}, unset: [] };
}

function isSaved(draft: EnvDraft, key: string): boolean {
	return draft.saved.some((e) => e.key === key);
}

/** Set `key` to `value`; `previousKey` renames a variable (its old key is removed). */
export function draftSet(
	draft: EnvDraft,
	key: string,
	value: string,
	previousKey: string | null = null
): EnvDraft {
	const set = { ...draft.set, [key]: value };
	let unset = draft.unset.filter((k) => k !== key);
	if (previousKey && previousKey !== key) {
		delete set[previousKey];
		if (isSaved(draft, previousKey) && !unset.includes(previousKey)) {
			unset = [...unset, previousKey];
		}
	}
	return { ...draft, set, unset };
}

export function draftRemove(draft: EnvDraft, key: string): EnvDraft {
	const set = { ...draft.set };
	delete set[key];
	const unset =
		isSaved(draft, key) && !draft.unset.includes(key) ? [...draft.unset, key] : draft.unset;
	return { ...draft, set, unset };
}

/** The variables as they will be after saving, sorted by key. */
export function draftRows(draft: EnvDraft): EnvDraftRow[] {
	const rows = new Map<string, EnvDraftRow>();
	for (const entry of draft.saved) {
		if (draft.unset.includes(entry.key)) continue;
		rows.set(entry.key, { key: entry.key, change: null, empty: entry.masked === '' });
	}
	for (const [key, value] of Object.entries(draft.set)) {
		rows.set(key, { key, change: isSaved(draft, key) ? 'changed' : 'new', empty: value === '' });
	}
	return [...rows.values()].sort((a, b) => a.key.localeCompare(b.key));
}

export function draftIsDirty(draft: EnvDraft): boolean {
	return Object.keys(draft.set).length > 0 || draft.unset.length > 0;
}

/** Request fields for the patch (empty when nothing changed). */
export function draftPatch(draft: EnvDraft): {
	env_set?: Record<string, string>;
	env_unset?: string[];
} {
	return {
		...(Object.keys(draft.set).length > 0 && { env_set: draft.set }),
		...(draft.unset.length > 0 && { env_unset: draft.unset })
	};
}
