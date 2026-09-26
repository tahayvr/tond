import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { goto, setupAdmin, checkSetupToken, setAuthenticated, pageState } = vi.hoisted(() => ({
	goto: vi.fn(),
	setupAdmin: vi.fn(),
	checkSetupToken: vi.fn(),
	setAuthenticated: vi.fn(),
	pageState: { url: new URL('http://localhost/setup?token=good-token') }
}));

vi.mock('$app/navigation', () => ({
	goto
}));

vi.mock('$app/state', () => ({
	page: pageState
}));

vi.mock('$lib/api', () => ({
	setupAdmin,
	checkSetupToken
}));

vi.mock('$lib/auth.svelte', () => ({
	setAuthenticated
}));

import SetupPage from '../../routes/setup/+page.svelte';

describe('setup page', () => {
	beforeEach(() => {
		goto.mockReset();
		setupAdmin.mockReset();
		checkSetupToken.mockReset();
		setAuthenticated.mockReset();
		checkSetupToken.mockResolvedValue({ valid: true });
		pageState.url = new URL('http://localhost/setup?token=good-token');
	});

	it('explains where to find the setup link when the URL has no token', async () => {
		pageState.url = new URL('http://localhost/setup');
		render(SetupPage);

		expect(await screen.findByText(/setup link required/i)).toBeInTheDocument();
		expect(screen.getByText('sudo tond setup-link')).toBeInTheDocument();
		expect(screen.queryByLabelText('email')).not.toBeInTheDocument();
		expect(checkSetupToken).not.toHaveBeenCalled();
	});

	it('accepts a pasted token when the link was lost', async () => {
		pageState.url = new URL('http://localhost/setup');
		const user = userEvent.setup();
		render(SetupPage);

		await user.type(await screen.findByLabelText(/paste the setup token/i), 'pasted-token');
		await user.click(screen.getByRole('button', { name: /continue/i }));

		expect(checkSetupToken).toHaveBeenCalledWith('pasted-token');
		expect(await screen.findByLabelText('email')).toBeInTheDocument();
	});

	it('shows a friendly error for an invalid or used link', async () => {
		checkSetupToken.mockRejectedValue(
			new Error('This setup link is invalid or has already been used.')
		);
		render(SetupPage);

		expect(await screen.findByText(/invalid or has already been used/i)).toBeInTheDocument();
		expect(screen.getByRole('link', { name: /sign in/i })).toHaveAttribute('href', '/login');
		expect(screen.queryByLabelText('email')).not.toBeInTheDocument();
	});

	it('shows the length-based policy without composition rules', async () => {
		const user = userEvent.setup();
		render(SetupPage);

		await user.type(await screen.findByLabelText('password'), 'purple otter lamp');

		expect(screen.getByText(/at least 12 characters/i)).toBeInTheDocument();
		expect(screen.getByText(/common or breached password/i)).toBeInTheDocument();
		expect(screen.queryByText(/uppercase/i)).not.toBeInTheDocument();
		expect(screen.queryByText(/special character/i)).not.toBeInTheDocument();
	});

	it('blocks submission when the password is too weak', async () => {
		const user = userEvent.setup();
		render(SetupPage);

		await user.type(await screen.findByLabelText('email'), 'admin@example.com');
		await user.type(screen.getByLabelText('password'), 'weak');
		await user.type(screen.getByLabelText(/confirm password/i), 'weak');
		await user.click(screen.getByRole('button', { name: /create admin account/i }));

		expect(
			await screen.findByText(/at least 12 characters\. a few random words/i)
		).toBeInTheDocument();
		expect(setupAdmin).not.toHaveBeenCalled();
	});

	it('blocks submission when passwords do not match', async () => {
		const user = userEvent.setup();
		render(SetupPage);

		await user.type(await screen.findByLabelText('email'), 'admin@example.com');
		await user.type(screen.getByLabelText('password'), 'StrongPassword1!');
		await user.type(screen.getByLabelText(/confirm password/i), 'StrongPassword2!');
		await user.click(screen.getByRole('button', { name: /create admin account/i }));

		expect(await screen.findByText(/passwords do not match/i)).toBeInTheDocument();
		expect(setupAdmin).not.toHaveBeenCalled();
	});

	it('creates the admin account with the token from the link and redirects home', async () => {
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

		await user.type(await screen.findByLabelText('email'), 'admin@example.com');
		await user.type(screen.getByLabelText('password'), 'StrongPassword1!');
		await user.type(screen.getByLabelText(/confirm password/i), 'StrongPassword1!');
		await user.click(screen.getByRole('button', { name: /create admin account/i }));

		expect(checkSetupToken).toHaveBeenCalledWith('good-token');
		expect(setupAdmin).toHaveBeenCalledWith('good-token', 'admin@example.com', 'StrongPassword1!');
		expect(setAuthenticated).toHaveBeenCalledWith(
			expect.objectContaining({ email: 'admin@example.com' })
		);
		expect(goto).toHaveBeenCalledWith('/', { replaceState: true });
	});
});
