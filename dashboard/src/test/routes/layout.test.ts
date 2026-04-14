import { describe, expect, it } from 'vitest';

describe('+layout route options', () => {
	it('disables SSR at the root layout for the static SPA dashboard', async () => {
		const layoutModule = await import('../../routes/+layout');

		expect(layoutModule.ssr).toBe(false);
	});

	it('does not prerender routes during build', async () => {
		const layoutModule = await import('../../routes/+layout');

		expect(layoutModule.prerender).toBe(false);
	});

	it('does not export a load function that re-checks auth on every navigation', async () => {
		const layoutModule = await import('../../routes/+layout');

		expect(layoutModule).not.toHaveProperty('load');
	});
});
