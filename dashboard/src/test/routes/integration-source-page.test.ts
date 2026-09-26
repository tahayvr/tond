import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const state = vi.hoisted(() => ({
	page: {
		url: new URL('http://localhost/integrations/github'),
		params: { source: 'github' } as Record<string, string>
	}
}));

vi.mock('$app/state', () => ({
	page: state.page
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));

vi.mock('$lib/components/integrations/github-app-list.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

import IntegrationSourcePage from '../../routes/integrations/[source]/+page.svelte';

function renderPage() {
	render(IntegrationSourcePage, {
		props: { params: { source: 'github' }, data: { sourceData: { githubApps: [] } } } as never
	});
}

describe('integrations/[source] page', () => {
	beforeEach(() => {
		state.page.params = { source: 'github' };
	});

	it('explains GitHub App callback errors', async () => {
		state.page.url = new URL('http://localhost/integrations/github?error=invalid_state');
		renderPage();

		expect(await screen.findByRole('alert')).toHaveTextContent(/expired \(1 hour\)/i);
	});

	it('shows no banner without an error', () => {
		state.page.url = new URL('http://localhost/integrations/github');
		renderPage();

		expect(screen.queryByRole('alert')).not.toBeInTheDocument();
	});
});
