import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { verify2FA, setAuthenticated } = vi.hoisted(() => ({
	verify2FA: vi.fn(),
	setAuthenticated: vi.fn()
}));

vi.mock('$lib/api', () => ({
	verify2FA
}));

vi.mock('$lib/auth.svelte', () => ({
	setAuthenticated
}));

vi.mock('$app/state', () => ({
	page: { url: new URL('http://localhost/login/2fa?next=%2Fapps%2Fapp_1') }
}));

import TwoFactorPage from '../../routes/login/2fa/+page.svelte';

// bits-ui's PIN input re-dispatches input events on 0/10/50 ms timers; let them
// finish before the DOM is torn down, or they fire into a disposed document.
afterEach(() => new Promise((resolve) => setTimeout(resolve, 60)));

describe('2fa login page', () => {
	beforeEach(() => {
		verify2FA.mockReset();
		setAuthenticated.mockReset();
	});

	it('verifies an authenticator code', async () => {
		verify2FA.mockResolvedValue({
			user: {
				id: 'usr_1',
				email: 'admin@example.com',
				role: 'admin',
				created_at: '2026-01-01T00:00:00.000Z'
			}
		});

		const user = userEvent.setup();
		render(TwoFactorPage);

		const otpInput = document.querySelector('input[autocomplete="one-time-code"]');
		expect(otpInput).not.toBeNull();

		await user.type(otpInput as HTMLInputElement, '123456');
		await user.click(screen.getByRole('button', { name: /verify/i }));

		expect(verify2FA).toHaveBeenCalledWith('123456');
		expect(setAuthenticated).toHaveBeenCalledWith(
			expect.objectContaining({ email: 'admin@example.com' })
		);
	});

	it('allows switching to backup code verification', async () => {
		verify2FA.mockResolvedValue({
			user: {
				id: 'usr_1',
				email: 'admin@example.com',
				role: 'admin',
				created_at: '2026-01-01T00:00:00.000Z'
			}
		});

		const user = userEvent.setup();
		render(TwoFactorPage);

		await user.click(screen.getByRole('button', { name: /use a backup code instead/i }));
		await user.type(screen.getByLabelText('backup-code'), 'abcd1234efgh5678');
		await user.click(screen.getByRole('button', { name: /verify/i }));

		expect(verify2FA).toHaveBeenCalledWith('abcd1234efgh5678');
	});

	it('shows an error when verification fails', async () => {
		verify2FA.mockRejectedValue(new Error('Invalid TOTP code'));

		const user = userEvent.setup();
		render(TwoFactorPage);

		const otpInput = document.querySelector('input[autocomplete="one-time-code"]');
		expect(otpInput).not.toBeNull();

		await user.type(otpInput as HTMLInputElement, '123456');
		await user.click(screen.getByRole('button', { name: /verify/i }));

		expect(await screen.findByText(/invalid totp code/i)).toBeInTheDocument();
		expect(setAuthenticated).not.toHaveBeenCalled();
	});

	it('links back to sign in, keeping the return target', () => {
		render(TwoFactorPage);

		expect(screen.getByRole('link', { name: /back to sign in/i })).toHaveAttribute(
			'href',
			'/login?next=%2Fapps%2Fapp_1'
		);
	});
});
