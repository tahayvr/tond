import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { goto, login, setAuthenticated } = vi.hoisted(() => ({
	goto: vi.fn(),
	login: vi.fn(),
	setAuthenticated: vi.fn()
}));

vi.mock('$app/navigation', () => ({
	goto
}));

vi.mock('$lib/api', () => ({
	login
}));

vi.mock('$lib/auth.svelte', () => ({
	setAuthenticated
}));

import LoginPage from '../../routes/login/+page.svelte';

describe('login page', () => {
	beforeEach(() => {
		goto.mockReset();
		login.mockReset();
		setAuthenticated.mockReset();
	});

	it('submits credentials and authenticates the user', async () => {
		login.mockResolvedValue({
			user: {
				id: 'usr_1',
				email: 'admin@example.com',
				role: 'admin',
				created_at: '2026-01-01T00:00:00.000Z'
			}
		});

		const user = userEvent.setup();
		render(LoginPage);

		await user.type(screen.getByLabelText('email'), 'admin@example.com');
		await user.type(screen.getByLabelText('password'), 'supersecret');
		await user.click(screen.getByRole('button', { name: /sign in/i }));

		expect(login).toHaveBeenCalledWith('admin@example.com', 'supersecret');
		expect(setAuthenticated).toHaveBeenCalledWith(
			expect.objectContaining({ email: 'admin@example.com' })
		);
		expect(goto).not.toHaveBeenCalled();
	});

	it('redirects to 2fa when the server requires MFA', async () => {
		login.mockResolvedValue({ mfa_required: true });

		const user = userEvent.setup();
		render(LoginPage);

		await user.type(screen.getByLabelText('email'), 'admin@example.com');
		await user.type(screen.getByLabelText('password'), 'supersecret');
		await user.click(screen.getByRole('button', { name: /sign in/i }));

		expect(goto).toHaveBeenCalledWith('/login/2fa');
		expect(setAuthenticated).not.toHaveBeenCalled();
	});

	it('shows an inline error when login fails', async () => {
		login.mockRejectedValue(new Error('Invalid credentials'));

		const user = userEvent.setup();
		render(LoginPage);

		await user.type(screen.getByLabelText('email'), 'admin@example.com');
		await user.type(screen.getByLabelText('password'), 'wrong-password');
		await user.click(screen.getByRole('button', { name: /sign in/i }));

		expect(await screen.findByText(/invalid credentials/i)).toBeInTheDocument();
		expect(setAuthenticated).not.toHaveBeenCalled();
	});
});
