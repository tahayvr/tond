import { describe, expect, it } from 'vitest';
import { buildPaletteEntries } from './command-palette';
import { DOCS_URL, docsUrl } from './docs';

describe('docsUrl', () => {
	it('builds links under tond.io/docs', () => {
		expect(DOCS_URL).toBe('https://tond.io/docs');
		expect(docsUrl()).toBe('https://tond.io/docs/');
		expect(docsUrl('/guides/upgrading')).toBe('https://tond.io/docs/guides/upgrading');
		expect(docsUrl('reference/cli')).toBe('https://tond.io/docs/reference/cli');
	});

	it('is offered in the command palette', () => {
		const entries = buildPaletteEntries({ apps: [], databases: [], cronJobs: [], projects: [] });
		const docs = entries.find((e) => e.id === 'act:docs');
		expect(docs?.action).toEqual({ kind: 'docs' });
		expect(docs?.keywords).toContain('help');
	});
});
