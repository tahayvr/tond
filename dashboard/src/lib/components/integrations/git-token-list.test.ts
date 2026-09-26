import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	createGiteaToken: vi.fn(),
	createGitLabToken: vi.fn(),
	deleteGiteaToken: vi.fn(),
	deleteGitLabToken: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});
vi.mock('$lib/auth.svelte', () => ({ can: () => true }));

import GitTokenList from './git-token-list.svelte';

describe('GitTokenList', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
	});

	it('adds a Gitea token after the server checked it', async () => {
		const user = userEvent.setup();
		api.createGiteaToken.mockResolvedValue({
			id: 'gt2',
			name: 'Codeberg',
			base_url: 'https://codeberg.org',
			username: 'me',
			created_at: '2026-09-01T00:00:00Z'
		});
		render(GitTokenList, { props: { kind: 'gitea' } });
		expect(screen.getByText(/No Gitea \/ Forgejo tokens yet/)).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /Add a token/ }));
		const save = screen.getByRole('button', { name: 'Save token' });
		expect(save).toBeDisabled();
		await user.type(screen.getByLabelText('Name'), 'Codeberg');
		await user.type(screen.getByLabelText('Instance URL'), 'https://codeberg.org');
		await user.type(screen.getByLabelText('Access token'), 's3cret');
		await user.click(save);

		await waitFor(() =>
			expect(api.createGiteaToken).toHaveBeenCalledWith({
				name: 'Codeberg',
				token: 's3cret',
				base_url: 'https://codeberg.org'
			})
		);
		expect(await screen.findByText(/https:\/\/codeberg.org · me/)).toBeInTheDocument();
	});

	it('shows why a GitLab token was refused', async () => {
		const user = userEvent.setup();
		api.createGitLabToken.mockRejectedValue(new Error('GitLab token validation failed (HTTP 401)'));
		render(GitTokenList, { props: { kind: 'gitlab' } });
		await user.click(screen.getByRole('button', { name: /Add a token/ }));
		expect(screen.getByLabelText('GitLab URL')).toHaveValue('https://gitlab.com');
		await user.type(screen.getByLabelText('Name'), 'Work');
		await user.type(screen.getByLabelText('Access token'), 'bad');
		await user.click(screen.getByRole('button', { name: 'Save token' }));
		expect(await screen.findByText(/validation failed/)).toBeInTheDocument();
	});
});
