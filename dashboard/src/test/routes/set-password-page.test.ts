import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	checkPasswordLink: vi.fn(),
	completePasswordLink: vi.fn()
}));
const nav = vi.hoisted(() => ({ goto: vi.fn(), search: '?token=good' }));
const auth = vi.hoisted(() => ({ setAuthenticated: vi.fn() }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});
vi.mock('$app/navigation', () => ({ goto: nav.goto }));
vi.mock('$app/state', () => ({
	page: {
		get url() {
			return new URL(`http://localhost/set-password${nav.search}`);
		}
	}
}));
vi.mock('$lib/auth.svelte', () => auth);

import SetPasswordPage from '../../routes/set-password/+page.svelte';

describe('set-password page', () => {
	beforeEach(() => {
		api.checkPasswordLink.mockReset();
		api.completePasswordLink.mockReset();
		nav.goto.mockReset();
		auth.setAuthenticated.mockReset();
		nav.search = '?token=good';
	});

	it('explains an invalid or used link', async () => {
		api.checkPasswordLink.mockRejectedValue(
			new Error('This link is invalid, has expired or was already used.')
		);
		render(SetPasswordPage);
		expect(await screen.findByText(/already used/i)).toBeInTheDocument();
		expect(screen.getByRole('link', { name: /sign in/i })).toHaveAttribute('href', '/login');
	});

	it('checks length locally, then sets the password and signs in', async () => {
		const user = userEvent.setup();
		api.checkPasswordLink.mockResolvedValue({
			email: 'new@example.com',
			purpose: 'invite',
			expires_at: new Date().toISOString()
		});
		api.completePasswordLink.mockResolvedValue({
			signed_in: true,
			user: {
				id: 'u',
				email: 'new@example.com',
				role: 'viewer',
				totp_enabled: false,
				created_at: ''
			}
		});
		render(SetPasswordPage);

		expect(await screen.findByText('new@example.com')).toBeInTheDocument();
		await user.type(screen.getByLabelText('Password'), 'short');
		expect(screen.getByText(/use at least 12 characters/i)).toBeInTheDocument();
		await user.clear(screen.getByLabelText('Password'));
		await user.type(screen.getByLabelText('Password'), 'violet otter lamp');
		await user.type(screen.getByLabelText('Confirm password'), 'violet otter lamp');
		await user.click(screen.getByRole('button', { name: /create account/i }));

		expect(api.completePasswordLink).toHaveBeenCalledWith('good', 'violet otter lamp');
		expect(auth.setAuthenticated).toHaveBeenCalled();
		expect(nav.goto).toHaveBeenCalledWith('/', { replaceState: true });
	});

	it('shows the server’s breached-password message', async () => {
		const user = userEvent.setup();
		api.checkPasswordLink.mockResolvedValue({ email: 'a@b.c', purpose: 'reset', expires_at: '' });
		api.completePasswordLink.mockRejectedValue(
			new Error("This password is too common: it's on lists of breached passwords.")
		);
		render(SetPasswordPage);

		await user.type(await screen.findByLabelText('Password'), 'Password1234!');
		await user.type(screen.getByLabelText('Confirm password'), 'Password1234!');
		await user.click(screen.getByRole('button', { name: /set new password/i }));

		expect(await screen.findByRole('alert')).toHaveTextContent(/too common/i);
	});
});
