import { describe, expect, it } from 'vitest';
import { safeUrl } from './utils';

describe('safeUrl', () => {
	it('keeps https URLs', () => {
		expect(safeUrl('https://github.com/org/repo/pull/7')).toBe(
			'https://github.com/org/repo/pull/7'
		);
		expect(safeUrl('  https://app.example.com  ')).toBe('https://app.example.com/');
	});

	it('drops script, data and other non-https URLs', () => {
		for (const url of [
			'javascript:alert(1)',
			'JavaScript:alert(1)',
			' javascript:alert(1)',
			'java\tscript:alert(1)',
			'data:text/html,<script>alert(1)</script>',
			'vbscript:msgbox(1)',
			'file:///etc/passwd',
			'//evil.example.com',
			'/relative/path',
			'http://github.com/x',
			'not a url',
			'',
			null,
			undefined
		]) {
			expect(safeUrl(url)).toBeUndefined();
		}
	});

	it('allows http only when asked', () => {
		expect(safeUrl('http://localhost:3000/', { allowHttp: true })).toBe('http://localhost:3000/');
		expect(safeUrl('javascript:alert(1)', { allowHttp: true })).toBeUndefined();
	});
});
