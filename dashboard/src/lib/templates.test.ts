import { describe, expect, it } from 'vitest';
import type { Template, TemplateInput } from '$lib/api';
import templatesJson from '../mocks/templates.json';
import {
	appHost,
	appNameError,
	categoryCounts,
	categoryLabel,
	expandPlaceholders,
	filterTemplates,
	formatRam,
	inputError,
	inputsToSend,
	memoryFit,
	memoryWarning
} from './templates';
import { TEMPLATE_LOGOS } from './components/templates/template-logos';

const catalog = templatesJson as Template[];

function input(overrides: Partial<TemplateInput> = {}): TemplateInput {
	return {
		key: 'X',
		label: 'X value',
		description: null,
		kind: 'text',
		required: false,
		generated: false,
		default: null,
		credential: false,
		...overrides
	};
}

describe('template catalog', () => {
	it('has a logo registered for every logo slug the templates use', () => {
		for (const t of catalog) {
			if (t.logo) expect(TEMPLATE_LOGOS[t.logo], `${t.id}: ${t.logo}`).toBeDefined();
		}
	});

	it('labels every category', () => {
		for (const t of catalog) expect(categoryLabel(t.category)).not.toBe(t.category);
	});

	it('counts categories, largest first', () => {
		const counts = categoryCounts(catalog);
		expect(counts.reduce((n, c) => n + c.count, 0)).toBe(catalog.length);
		for (let i = 1; i < counts.length; i++) {
			expect(counts[i - 1].count).toBeGreaterThanOrEqual(counts[i].count);
		}
	});

	it('searches names, descriptions and tags, name matches first', () => {
		const all = { query: '', category: '', fitsOnly: false };
		expect(filterTemplates(catalog, all)).toHaveLength(catalog.length);
		const git = filterTemplates(catalog, { ...all, query: 'git' }).map((t) => t.id);
		expect(git).toContain('forgejo');
		expect(git).toContain('gitea');
		expect(git.indexOf('gitea')).toBeLessThan(git.indexOf('forgejo'));
		expect(
			filterTemplates(catalog, { ...all, query: 'google analytics' }).map((t) => t.id)
		).toEqual(expect.arrayContaining(['umami', 'plausible']));
		expect(filterTemplates(catalog, { ...all, query: 'zzzz' })).toEqual([]);
	});

	it('filters by category and the 2 GB badge', () => {
		const monitoring = filterTemplates(catalog, {
			query: '',
			category: 'monitoring',
			fitsOnly: false
		});
		expect(monitoring.every((t) => t.category === 'monitoring')).toBe(true);
		const small = filterTemplates(catalog, { query: '', category: '', fitsOnly: true });
		expect(small.every((t) => t.fits_2gb)).toBe(true);
		expect(small.map((t) => t.id)).not.toContain('immich');
	});
});

describe('memory', () => {
	it('formats sizes', () => {
		expect(formatRam(300)).toBe('300 MB');
		expect(formatRam(1024)).toBe('1 GB');
		expect(formatRam(1536)).toBe('1.5 GB');
	});

	it('compares needs with free memory', () => {
		const memory = { total_mb: 2048, available_mb: 1000 };
		expect(memoryFit({ ram_mb: 300 }, memory)).toBe('fits');
		expect(memoryFit({ ram_mb: 900 }, memory)).toBe('tight');
		expect(memoryFit({ ram_mb: 1500 }, memory)).toBe('short');
		expect(memoryFit({ ram_mb: 1500 }, null)).toBe('unknown');
	});

	it('warns in plain words only when it matters', () => {
		const memory = { total_mb: 2048, available_mb: 1000 };
		expect(memoryWarning({ name: 'Umami', ram_mb: 300 }, memory)).toBeNull();
		expect(memoryWarning({ name: 'Immich', ram_mb: 1500 }, memory)).toMatch(
			/1000 MB of memory free and Immich needs about 1.5 GB/
		);
		expect(memoryWarning({ name: 'Metabase', ram_mb: 900 }, memory)).toMatch(/tight/);
	});
});

describe('deploy form values', () => {
	it('checks app names like the server', () => {
		expect(appNameError('umami')).toBeNull();
		expect(appNameError('')).toMatch(/name/);
		expect(appNameError('My App')).toMatch(/lowercase/);
		expect(appNameError('-x')).toMatch(/hyphen/);
		expect(appNameError('a'.repeat(41))).toMatch(/40/);
	});

	it('works out the address', () => {
		expect(appHost('stats', '', 'apps.example.com')).toBe('stats.apps.example.com');
		expect(appHost('stats', 'Stats.Example.com ', 'apps.example.com')).toBe('stats.example.com');
		expect(appHost('stats', '', '')).toBeNull();
		expect(expandPlaceholders('{app_url}/x {app_name}', 'a', 'a.example.com')).toBe(
			'https://a.example.com/x a'
		);
		expect(expandPlaceholders('{app_url}', 'a', 'a.localhost')).toBe('http://a.localhost');
	});

	it('validates inputs by kind', () => {
		expect(inputError(input({ required: true }), '')).toMatch(/required/);
		expect(inputError(input(), '')).toBeNull();
		expect(inputError(input({ kind: 'email' }), 'nope')).toMatch(/email/);
		expect(inputError(input({ kind: 'email' }), 'me@example.com')).toBeNull();
		expect(inputError(input({ kind: 'url' }), 'example.com')).toMatch(/https/);
		expect(inputError(input({ kind: 'password' }), 'short')).toMatch(/8 characters/);
	});

	it('sends only values the user changed or typed', () => {
		const inputs = [
			input({ key: 'TZ', default: 'UTC' }),
			input({ key: 'EMAIL', required: true }),
			input({ key: 'PASSWORD', generated: true })
		];
		expect(inputsToSend(inputs, { TZ: 'UTC', EMAIL: ' me@example.com ', PASSWORD: '' })).toEqual({
			EMAIL: 'me@example.com'
		});
		expect(
			inputsToSend(inputs, { TZ: 'Europe/Berlin', EMAIL: 'x', PASSWORD: 'mine-12345' })
		).toEqual({ TZ: 'Europe/Berlin', EMAIL: 'x', PASSWORD: 'mine-12345' });
	});
});
