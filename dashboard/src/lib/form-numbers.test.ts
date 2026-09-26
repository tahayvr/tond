import { describe, expect, it } from 'vitest';

import { intOr, optionalInt } from './form-numbers';

describe('optionalInt', () => {
	it('accepts the numbers Svelte binds from number inputs', () => {
		expect(optionalInt(300)).toBe(300);
		expect(optionalInt(0)).toBe(0);
		expect(optionalInt(12.9)).toBe(12);
	});

	it('treats empty fields as null', () => {
		expect(optionalInt(null)).toBeNull();
		expect(optionalInt(undefined)).toBeNull();
		expect(optionalInt('')).toBeNull();
		expect(optionalInt('   ')).toBeNull();
	});

	it('still accepts numeric strings and rejects garbage', () => {
		expect(optionalInt(' 42 ')).toBe(42);
		expect(optionalInt('abc')).toBeNull();
		expect(optionalInt(Number.NaN)).toBeNull();
	});
});

describe('intOr', () => {
	it('falls back only when the field is empty or invalid', () => {
		expect(intOr(null, 60)).toBe(60);
		expect(intOr(0, 60)).toBe(0);
		expect(intOr(5, 60)).toBe(5);
	});
});
