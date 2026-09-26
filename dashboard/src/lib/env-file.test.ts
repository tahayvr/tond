import { describe, expect, it } from 'vitest';

import { isValidEnvKey, parseDotenv, planEnvImport } from './env-file';

describe('parseDotenv', () => {
	it('parses plain, exported, quoted and commented lines', () => {
		const result = parseDotenv(
			[
				'# database settings',
				'',
				'DATABASE_URL=postgres://user:pass@db/app',
				'export PORT = 8080',
				'GREETING="hello \\"world\\"\\nbye"',
				"LITERAL='$HOME stays #literal'",
				'WITH_COMMENT=value # trailing comment',
				'HASH_IN_VALUE=abc#def',
				'EMPTY=',
				'EMPTY_QUOTED=""'
			].join('\n')
		);

		expect(result.errors).toEqual([]);
		expect(result.vars).toEqual({
			DATABASE_URL: 'postgres://user:pass@db/app',
			PORT: '8080',
			GREETING: 'hello "world"\nbye',
			LITERAL: '$HOME stays #literal',
			WITH_COMMENT: 'value',
			HASH_IN_VALUE: 'abc#def',
			EMPTY: '',
			EMPTY_QUOTED: ''
		});
	});

	it('supports multi-line quoted values and CRLF input', () => {
		const result = parseDotenv('KEY="-----BEGIN-----\r\nabc\r\n-----END-----"\r\nNEXT=1');

		expect(result.errors).toEqual([]);
		expect(result.vars).toEqual({ KEY: '-----BEGIN-----\nabc\n-----END-----', NEXT: '1' });
	});

	it('reports invalid lines with their line numbers and keeps the valid ones', () => {
		const result = parseDotenv(
			['GOOD=1', 'no equals sign', '1BAD=2', 'OPEN="never closed', 'ALSO_GOOD=3'].join('\n')
		);

		expect(result.vars).toEqual({ GOOD: '1' });
		expect(result.errors.map((e) => e.line)).toEqual([2, 3, 4]);
		expect(result.errors[2].message).toMatch(/closing/);
	});

	it('lets a later duplicate key win', () => {
		expect(parseDotenv('A=1\nA=2').vars).toEqual({ A: '2' });
	});
});

describe('planEnvImport', () => {
	it('splits incoming keys into added and overwritten, sorted', () => {
		expect(planEnvImport(['B', 'C'], { C: 'x', A: 'y', B: 'z' })).toEqual({
			added: ['A'],
			overwritten: ['B', 'C']
		});
	});
});

describe('isValidEnvKey', () => {
	it('accepts POSIX-style names only', () => {
		expect(isValidEnvKey('API_KEY')).toBe(true);
		expect(isValidEnvKey('_x1')).toBe(true);
		expect(isValidEnvKey('1X')).toBe(false);
		expect(isValidEnvKey('A-B')).toBe(false);
		expect(isValidEnvKey('')).toBe(false);
	});
});
