import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ApiToken } from '$lib/api';
import { MOCK_TOKEN_CATALOG } from '../../mocks/fixtures';

const api = vi.hoisted(() => ({
	listTokens: vi.fn(),
	revokeToken: vi.fn(),
	createToken: vi.fn(),
	getTokenCatalog: vi.fn(),
	listApps: vi.fn(),
	listProjects: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});
vi.mock('$lib/auth.svelte', () => ({ can: () => true }));

import TokenList from '$lib/components/tokens/token-list.svelte';

const token = (extra: Partial<ApiToken> = {}): ApiToken => ({
	id: 'tok_1',
	name: 'github-actions',
	prefix: 'tond_pat_12345678',
	kind: 'ci',
	scopes: ['apps:read', 'apps:deploy'],
	app_ids: ['app_1'],
	project_ids: [],
	allowed_ips: [],
	owner_id: 'usr_1',
	owner_label: 'a@b.c',
	expires_at: new Date(Date.now() + 60 * 86400_000).toISOString(),
	last_used_at: new Date(Date.now() - 5 * 60_000).toISOString(),
	last_used_ip: '140.82.112.4',
	revoked_at: null,
	created_at: '2026-01-01T00:00:00Z',
	...extra
});

describe('API tokens', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		document.body.style.pointerEvents = '';
		api.getTokenCatalog.mockResolvedValue(MOCK_TOKEN_CATALOG);
		api.listApps.mockResolvedValue([{ id: 'app_1', name: 'web' }]);
		api.listProjects.mockResolvedValue([]);
	});

	it('lists tokens with last use and hides revoked ones by default', () => {
		render(TokenList, {
			props: {
				initialTokens: [
					token(),
					token({ id: 'tok_2', name: 'old', revoked_at: '2026-01-02T00:00:00Z' })
				]
			}
		});
		expect(screen.getByText('github-actions')).toBeInTheDocument();
		expect(screen.getByText(/last used 5 min ago from 140\.82\.112\.4/i)).toBeInTheDocument();
		expect(screen.getByText(/limited to 1 app/i)).toBeInTheDocument();
		expect(screen.queryByText('old')).not.toBeInTheDocument();
		expect(screen.getByText(/show revoked and expired \(1\)/i)).toBeInTheDocument();
	});

	it('asks before revoking and then marks the token revoked', async () => {
		const user = userEvent.setup();
		api.revokeToken.mockResolvedValue(token({ revoked_at: new Date().toISOString() }));
		render(TokenList, { props: { initialTokens: [token()] } });

		await user.click(screen.getByRole('button', { name: 'Revoke' }));
		const dialog = await screen.findByRole('alertdialog');
		expect(within(dialog).getByText(/stops working immediately/i)).toBeInTheDocument();
		await user.click(within(dialog).getByRole('button', { name: 'Revoke token' }));

		expect(api.revokeToken).toHaveBeenCalledWith('tok_1');
		expect(await screen.findByText(/no active tokens/i)).toBeInTheDocument();
	});

	it('creates a token from a preset and shows the secret exactly once', async () => {
		const user = userEvent.setup();
		const created = token({ id: 'tok_new', name: 'deploy-bot' });
		api.createToken.mockResolvedValue({ token: created, secret: `tond_pat_${'a'.repeat(64)}` });
		render(TokenList, { props: { initialTokens: [] } });

		await user.click(screen.getByRole('button', { name: /new token/i }));
		await user.type(await screen.findByLabelText('Name'), 'deploy-bot');
		await user.click(screen.getByRole('button', { name: /deployer \(ci\)/i }));
		await user.click(screen.getByRole('button', { name: 'Create token' }));

		expect(api.createToken).toHaveBeenCalledWith(
			expect.objectContaining({
				name: 'deploy-bot',
				scopes: ['apps:read', 'apps:deploy', 'logs:read'],
				expires_in_hours: 90 * 24
			})
		);
		expect(await screen.findByText(/store it now/i)).toBeInTheDocument();
		expect(screen.getByTestId('token-secret')).toHaveTextContent(`tond_pat_${'a'.repeat(64)}`);
		await user.click(screen.getByRole('button', { name: /i've stored it/i }));
		expect(screen.queryByTestId('token-secret')).not.toBeInTheDocument();
		expect(screen.getByText('deploy-bot')).toBeInTheDocument();
	});

	it('explains what is missing instead of submitting an incomplete form', async () => {
		const user = userEvent.setup();
		render(TokenList, { props: { initialTokens: [] } });

		await user.click(screen.getByRole('button', { name: /new token/i }));
		await user.click(await screen.findByRole('button', { name: 'Create token' }));

		expect(await screen.findByText(/give the token a name/i)).toBeInTheDocument();
		expect(api.createToken).not.toHaveBeenCalled();
	});
});
