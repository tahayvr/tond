import { describe, expect, it } from 'vitest';
import {
	draftIsDirty,
	draftPatch,
	draftRemove,
	draftRows,
	draftSet,
	envDraftFrom
} from './env-draft';

const MASK = '••••••••';
const saved = [
	{ key: 'API_KEY', masked: MASK },
	{ key: 'EMPTY', masked: '' }
];

describe('env drafts', () => {
	it('starts clean and lists saved keys without values', () => {
		const draft = envDraftFrom(saved);
		expect(draftIsDirty(draft)).toBe(false);
		expect(draftPatch(draft)).toEqual({});
		expect(draftRows(draft)).toEqual([
			{ key: 'API_KEY', change: null, empty: false },
			{ key: 'EMPTY', change: null, empty: true }
		]);
	});

	it('replaces, adds and removes only the named keys', () => {
		let draft = envDraftFrom(saved);
		draft = draftSet(draft, 'API_KEY', 'new-secret');
		draft = draftSet(draft, 'NEW', 'x');
		draft = draftRemove(draft, 'EMPTY');
		expect(draftPatch(draft)).toEqual({
			env_set: { API_KEY: 'new-secret', NEW: 'x' },
			env_unset: ['EMPTY']
		});
		expect(draftRows(draft).map((r) => [r.key, r.change])).toEqual([
			['API_KEY', 'changed'],
			['NEW', 'new']
		]);
	});

	it('renames by removing the old key and setting the new one', () => {
		const draft = draftSet(envDraftFrom(saved), 'TOKEN', 'v', 'API_KEY');
		expect(draftPatch(draft)).toEqual({ env_set: { TOKEN: 'v' }, env_unset: ['API_KEY'] });
	});

	it('removing an unsaved key just drops it', () => {
		let draft = draftSet(envDraftFrom(saved), 'NEW', 'x');
		draft = draftRemove(draft, 'NEW');
		expect(draftPatch(draft)).toEqual({});
	});
});
