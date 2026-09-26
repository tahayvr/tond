import { describe, expect, it } from 'vitest';
import {
	EMPTY_FILTERS,
	filtersFromUrl,
	filtersToQuery,
	filtersToSearch,
	hasFilters
} from './activity-filters';

describe('activity filters', () => {
	it('round-trips through the URL', () => {
		const filters = {
			q: 'web',
			actor: 'ci',
			action: 'login*',
			resourceType: 'app',
			failuresOnly: true
		};
		const search = filtersToSearch(filters);
		expect(search).toBe('?q=web&actor=ci&action=login*&resource_type=app&outcome=failure');
		expect(filtersFromUrl(new URL(`http://x/activity${search}`))).toEqual(filters);
	});

	it('produces an empty search and no query params without filters', () => {
		expect(filtersToSearch(EMPTY_FILTERS)).toBe('');
		expect(hasFilters(EMPTY_FILTERS)).toBe(false);
		expect(filtersToQuery(EMPTY_FILTERS)).toEqual({
			q: undefined,
			actor: undefined,
			action: undefined,
			resource_type: undefined,
			outcome: undefined,
			before: undefined,
			limit: 50
		});
	});

	it('passes the pagination cursor and trims text', () => {
		const query = filtersToQuery({ ...EMPTY_FILTERS, actor: '  a@b.c ' }, 120);
		expect(query.actor).toBe('a@b.c');
		expect(query.before).toBe(120);
	});
});
