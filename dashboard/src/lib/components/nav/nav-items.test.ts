import { describe, expect, it, vi } from 'vitest';

const auth = vi.hoisted(() => ({ scopes: [] as string[] }));
vi.mock('$lib/auth.svelte', () => ({ can: (scope: string) => auth.scopes.includes(scope) }));

import { visibleNavItems } from './nav-items.svelte';

describe('visibleNavItems', () => {
	it('hides admin pages from users without the scope', () => {
		auth.scopes = ['apps:read'];
		const hrefs = visibleNavItems().map((i) => i.href);
		expect(hrefs).toContain('/apps');
		expect(hrefs).toContain('/activity');
		expect(hrefs).not.toContain('/users');
		expect(hrefs).not.toContain('/integrations');
	});

	it('shows them to admins', () => {
		auth.scopes = ['users:admin', 'settings:write'];
		const hrefs = visibleNavItems().map((i) => i.href);
		expect(hrefs).toContain('/users');
		expect(hrefs).toContain('/integrations');
	});
});
