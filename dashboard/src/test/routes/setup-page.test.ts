import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { goto, setupAdmin, setAuthenticated } = vi.hoisted(() => ({
	goto: vi.fn(),
	setupAdmin: vi.fn(),
	setAuthenticated: vi.fn()
}));

vi.mock('$app/navigation', () => ({
	goto
}));

vi.mock('$lib/api', () => ({
	setupAdmin
}));

vi.mock('$lib/auth.svelte', () => ({
	setAuthenticated
}));

import SetupPage from '../../routes/setup/+page.svelte';

describe('setup page', () => {
	beforeEach(() => {
		goto.mockReset();
		setupAdmin.mockReset();
		setAuthenticated.mockReset();
	});

	it('shows password requirement feedback as the password becomes valid', async () => {
		const user = userEvent.setup();
		render(SetupPage);

		await user.type(screen.getByLabelText('password'), 'StrongPassword1!');

		expect(screen.getByText(/at least 12 characters/i)).toBeInTheDocument();
		expect(screen.getByText(/one uppercase letter/i)).toBeInTheDocument();
		expect(screen.getByText(/one lowercase letter/i)).toBeInTheDocument();
		expect(screen.getByText(/one number/i)).toBeInTheDocument();
		expect(screen.getByText(/one special character/i)).toBeInTheDocument();
	});

	it('blocks submission when the password is too weak', async () => {
		const user = userEvent.setup();
		render(SetupPage);

		await user.type(screen.getByLabelText('email'), 'admin@example.com');
		await user.type(screen.getByLabelText('password'), 'weak');
		await user.type(screen.getByLabelText(/confirm password/i), 'weak');
		await user.click(screen.getByRole('button', { name: /create admin account/i }));

		expect(await screen.findByText(/password does not meet all requirements/i)).toBeInTheDocument();
		expect(setupAdmin).not.toHaveBeenCalled();
	});

	it('blocks submission when passwords do not match', async () => {
		const user = userEvent.setup();
		render(SetupPage);

		await user.type(screen.getByLabelText('email'), 'admin@example.com');
		await user.type(screen.getByLabelText('password'), 'StrongPassword1!');
		await user.type(screen.getByLabelText(/confirm password/i), 'StrongPassword2!');
		await user.click(screen.getByRole('button', { name: /create admin account/i }));

		expect(await screen.findByText(/passwords do not match/i)).toBeInTheDocument();
		expect(setupAdmin).not.toHaveBeenCalled();
	});

	it('creates the admin account and redirects home', async () => {
		setupAdmin.mockResolvedValue({
			user: {
				id: 'usr_1',
				email: 'admin@example.com',
				role: 'admin',
				created_at: '2026-01-01T00:00:00.000Z'
			}
		});

		const user = userEvent.setup();
		render(SetupPage);

		await user.type(screen.getByLabelText('email'), 'admin@example.com');
		await user.type(screen.getByLabelText('password'), 'StrongPassword1!');
		await user.type(screen.getByLabelText(/confirm password/i), 'StrongPassword1!');
		await user.click(screen.getByRole('button', { name: /create admin account/i }));

		expect(setupAdmin).toHaveBeenCalledWith('admin@example.com', 'StrongPassword1!');
		expect(setAuthenticated).toHaveBeenCalledWith(
			expect.objectContaining({ email: 'admin@example.com' })
		);
		expect(goto).toHaveBeenCalledWith('/');
	});
});
