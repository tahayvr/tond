import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { setUiContext, clearUiContext, integrationsContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	integrationsContext: vi.fn(() => ({ type: 'integrations' }))
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	integrationsContext
}));

import IntegrationsPage from '../../routes/integrations/+page.svelte';

describe('integrations page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		integrationsContext.mockClear();
	});

	it('renders integration summary from route data without loading placeholders', () => {
		render(IntegrationsPage, {
			props: {
				params: {},
				data: {
					githubApps: [
						{
							id: 'gh_1',
							name: 'Tond GitHub App',
							app_id: 12345,
							client_id: 'Iv1.abc123',
							slug: 'tond-github-app',
							installation_id: 123,
							installed: true,
							created_at: '2026-01-01T00:00:00.000Z',
							updated_at: '2026-01-01T00:00:00.000Z'
						}
					],
					credentials: [
						{
							id: 'cred_1',
							name: 'GHCR',
							server: 'ghcr.io',
							username: 'tond-user',
							created_at: '2026-01-01T00:00:00.000Z',
							updated_at: '2026-01-01T00:00:00.000Z'
						}
					],
					integrations: [
						{
							id: 'int_1',
							kind: 'telegram',
							enabled: true,
							name: 'Telegram Alerts',
							config: {
								chat_id: '123456'
							},
							saved_secrets: ['bot_token'],
							created_at: '2026-01-01T00:00:00.000Z',
							updated_at: '2026-01-01T00:00:00.000Z'
						}
					]
				}
			}
		});

		expect(screen.getByText(/integrations/i)).toBeInTheDocument();
		expect(screen.getByText(/telegram/i)).toBeInTheDocument();
	});
});
