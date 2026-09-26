import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { confirmDialog, renderConfirmHost } from '../../../test/confirm';
import RegistryCredentialList from './registry-credential-list.svelte';

const api = vi.hoisted(() => ({
	listRegistryCredentials: vi.fn(),
	createRegistryCredential: vi.fn(),
	deleteRegistryCredential: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return {
		...actual,
		...api
	};
});

describe('RegistryCredentialList', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) {
			fn.mockReset();
		}
		api.createRegistryCredential.mockImplementation(async ({ name, server, username }) => ({
			id: 'cred_new',
			name,
			server,
			username,
			created_at: '2026-01-02T00:00:00.000Z',
			updated_at: '2026-01-02T00:00:00.000Z'
		}));
		api.deleteRegistryCredential.mockResolvedValue(undefined);
	});

	it('renders route-provided credentials without loading on mount', () => {
		render(RegistryCredentialList, {
			props: {
				initialCredentials: [
					{
						id: 'cred_1',
						name: 'GHCR',
						server: 'ghcr.io',
						username: 'tond-user',
						created_at: '2026-01-01T00:00:00.000Z',
						updated_at: '2026-01-01T00:00:00.000Z'
					}
				]
			}
		});

		expect(screen.queryByText(/loading/i)).not.toBeInTheDocument();
		expect(screen.getByText(/^GHCR$/)).toBeInTheDocument();
		expect(api.listRegistryCredentials).not.toHaveBeenCalled();
	});

	it('adds a credential using a preset server', async () => {
		const user = userEvent.setup();
		render(RegistryCredentialList, {
			props: {
				initialCredentials: []
			}
		});

		await user.click(screen.getByRole('button', { name: /add credential/i }));
		await user.type(screen.getByLabelText(/^name$/i), 'GHCR');
		await user.click(screen.getByRole('button', { name: /^ghcr$/i }));
		await user.type(screen.getByLabelText(/username/i), 'tond-user');
		await user.type(screen.getByLabelText(/password/i), 'secret-token');
		await user.click(screen.getByRole('button', { name: /save credential/i }));

		expect(api.createRegistryCredential).toHaveBeenCalledWith({
			name: 'GHCR',
			server: 'ghcr.io',
			username: 'tond-user',
			password: 'secret-token'
		});
		expect(await screen.findByText(/^GHCR$/)).toBeInTheDocument();
	});

	it('deletes a credential', async () => {
		const user = userEvent.setup();
		render(RegistryCredentialList, {
			props: {
				initialCredentials: [
					{
						id: 'cred_1',
						name: 'GHCR',
						server: 'ghcr.io',
						username: 'tond-user',
						created_at: '2026-01-01T00:00:00.000Z',
						updated_at: '2026-01-01T00:00:00.000Z'
					}
				]
			}
		});

		renderConfirmHost();
		await user.click(screen.getByRole('button', { name: /delete ghcr/i }));
		await confirmDialog(user);

		expect(api.deleteRegistryCredential).toHaveBeenCalledWith('cred_1');
		expect(screen.queryByText(/^GHCR$/)).not.toBeInTheDocument();
	});
});
