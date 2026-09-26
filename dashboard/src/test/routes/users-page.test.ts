import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MOCK_USERS } from '../../mocks/fixtures';

const api = vi.hoisted(() => ({
	listUsers: vi.fn(),
	updateUser: vi.fn(),
	deleteUser: vi.fn(),
	createPasswordLink: vi.fn(),
	listUserSessions: vi.fn(),
	revokeUserSessions: vi.fn(),
	revokeUserSession: vi.fn(),
	inviteUser: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});
vi.mock('$lib/auth.svelte', () => ({ can: () => true }));
vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));

import UsersPage from '../../routes/users/+page.svelte';

function renderPage() {
	render(UsersPage, { props: { params: {}, data: { users: structuredClone(MOCK_USERS) } } });
}

describe('users page', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		document.body.style.pointerEvents = '';
		api.listUsers.mockResolvedValue(structuredClone(MOCK_USERS));
	});

	it('shows each user with their status', () => {
		renderPage();
		expect(screen.getByText('dana@example.com')).toBeInTheDocument();
		expect(screen.getByText('Invited')).toBeInTheDocument();
		expect(screen.getByText('Disabled')).toBeInTheDocument();
		expect(screen.getByText('you')).toBeInTheDocument();
	});

	it('confirms before disabling and explains the consequences', async () => {
		const user = userEvent.setup();
		api.updateUser.mockResolvedValue({});
		renderPage();

		const row = screen.getByText('dana@example.com').closest('li') as HTMLElement;
		await user.click(within(row).getByRole('button', { name: 'Disable' }));
		const dialog = await screen.findByRole('alertdialog');
		expect(within(dialog).getByText(/signed out everywhere/i)).toBeInTheDocument();
		await user.click(within(dialog).getByRole('button', { name: 'Disable account' }));

		expect(api.updateUser).toHaveBeenCalledWith('usr_2', { disabled: true });
		expect(api.listUsers).toHaveBeenCalled();
	});

	it('never offers to disable or delete yourself', () => {
		renderPage();
		const row = screen.getByText('admin@localhost').closest('li') as HTMLElement;
		expect(within(row).queryByRole('button', { name: 'Disable' })).not.toBeInTheDocument();
		expect(within(row).queryByRole('button', { name: 'Delete' })).not.toBeInTheDocument();
	});

	it('hands out a reset link to copy', async () => {
		const user = userEvent.setup();
		api.createPasswordLink.mockResolvedValue({
			user: MOCK_USERS[1],
			token: 'abc',
			path: '/set-password?token=abc',
			expires_at: new Date(Date.now() + 86400_000).toISOString()
		});
		renderPage();

		const row = screen.getByText('dana@example.com').closest('li') as HTMLElement;
		await user.click(within(row).getByRole('button', { name: 'Reset password' }));

		expect(await screen.findByTestId('password-link')).toHaveTextContent('/set-password?token=abc');
	});
});
