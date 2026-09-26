import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	changePassword: vi.fn(),
	setup2FA: vi.fn(),
	confirm2FA: vi.fn(),
	disable2FA: vi.fn(),
	listSessions: vi.fn(),
	revokeSession: vi.fn(),
	revokeAllSessions: vi.fn()
}));

const { setUiContext, clearUiContext, projectsContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	projectsContext
}));

vi.mock('$lib/api', () => ({
	...api
}));

vi.mock('$app/state', async () => ({
	page: (await import('../mocks/page-state.svelte')).page
}));

vi.mock('$lib/components/tabs/profile-page/profile-two-factor.svelte', async () => ({
	default: (await import('../mocks/ProfileTwoFactorHarness.svelte')).default
}));

const { setAuthenticated } = vi.hoisted(() => ({ setAuthenticated: vi.fn() }));

vi.mock('$lib/auth.svelte', () => ({
	getUser: () => ({ email: 'admin@example.com', role: 'admin', totp_enabled: false }),
	can: () => true,
	setAuthenticated
}));

import ProfilePage from '../../routes/profile/+page.svelte';
import { setPageUrl } from '../mocks/page-state.svelte';
import { acknowledgeBackupCodes, getPendingBackupCodes } from '$lib/backup-codes.svelte';
import { confirmDialog, renderConfirmHost } from '../confirm';

const sessions = [
	{
		id: 'sess_1',
		created_at: '2026-01-01T00:00:00.000Z',
		expires_at: '2026-02-01T00:00:00.000Z',
		is_current: true,
		ip_address: '127.0.0.1',
		user_agent: 'Chrome',
		last_used_at: '2026-01-01T00:00:00.000Z'
	},
	{
		id: 'sess_2',
		created_at: '2026-01-02T00:00:00.000Z',
		expires_at: '2026-02-02T00:00:00.000Z',
		is_current: false,
		ip_address: '10.0.0.2',
		user_agent: 'Firefox',
		last_used_at: '2026-01-02T00:00:00.000Z'
	}
];

const me = {
	id: 'usr_1',
	email: 'admin@example.com',
	role: 'admin' as const,
	totp_enabled: false,
	created_at: '2026-01-01T00:00:00.000Z'
};

function renderPage(routeSessions = sessions, totpEnabled = false, tab = '') {
	setPageUrl(`http://localhost/profile${tab ? `?tab=${tab}` : ''}`);
	renderConfirmHost();
	render(ProfilePage, {
		props: {
			params: {},
			data: {
				sessions: routeSessions,
				me: { ...me, totp_enabled: totpEnabled },
				tokens: []
			}
		}
	});
}

describe('profile page', () => {
	beforeEach(() => {
		acknowledgeBackupCodes();
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();
		for (const fn of Object.values(api)) {
			fn.mockReset();
		}
		api.changePassword.mockResolvedValue(undefined);
		api.setup2FA.mockResolvedValue({ qr_code: 'ZmFrZQ==', secret: 'SECRET123' });
		api.confirm2FA.mockResolvedValue({ backup_codes: ['code-1', 'code-2', 'code-3'] });
		api.disable2FA.mockResolvedValue(undefined);
		api.listSessions.mockResolvedValue(sessions);
		api.revokeSession.mockResolvedValue(undefined);
		api.revokeAllSessions.mockResolvedValue(undefined);
	});

	it('renders profile shell from route data without initial session loading', () => {
		renderPage();

		expect(screen.getByText(/admin@example.com/i)).toBeInTheDocument();
	});

	it('changes the password when the form is valid', async () => {
		const user = userEvent.setup();
		renderPage();

		await user.type(screen.getByLabelText(/current password/i), 'old-password');
		await user.type(screen.getByLabelText(/^new password$/i), 'new-password-123');
		await user.type(screen.getByLabelText(/confirm new password/i), 'new-password-123');
		await user.click(screen.getByRole('button', { name: /change password/i }));

		expect(api.changePassword).toHaveBeenCalledWith('old-password', 'new-password-123');
		expect(await screen.findByText(/saved/i)).toBeInTheDocument();
	});

	it('shows a validation error when new passwords do not match', async () => {
		const user = userEvent.setup();
		renderPage();

		await user.type(screen.getByLabelText(/current password/i), 'old-password');
		await user.type(screen.getByLabelText(/^new password$/i), 'new-password-123');
		await user.type(screen.getByLabelText(/confirm new password/i), 'different-password');
		await user.click(screen.getByRole('button', { name: /change password/i }));

		expect(await screen.findByText(/new passwords do not match/i)).toBeInTheDocument();
		expect(api.changePassword).not.toHaveBeenCalled();
	});

	it('enables 2fa and confirms setup with backup codes', async () => {
		const user = userEvent.setup();
		renderPage(sessions, false, '2fa');

		await user.click(screen.getByRole('button', { name: /enable two-factor auth/i }));

		// The current password is required before a new authenticator is enrolled.
		expect(api.setup2FA).not.toHaveBeenCalled();
		await user.type(screen.getByLabelText(/confirm password/i), 'my-password');
		await user.click(screen.getByRole('button', { name: /continue/i }));

		expect(api.setup2FA).toHaveBeenCalledWith('my-password');
		expect(await screen.findByAltText(/totp qr code/i)).toBeInTheDocument();
		expect(screen.getByText(/secret123/i)).toBeInTheDocument();

		await user.type(screen.getByLabelText(/enter one-time password/i), '123456');
		await user.click(screen.getByRole('button', { name: /confirm & enable/i }));

		expect(api.confirm2FA).toHaveBeenCalledWith('123456');
		expect(await screen.findByText(/save these backup codes/i)).toBeInTheDocument();
		expect(screen.getByText(/code-1/i)).toBeInTheDocument();
		// The codes are kept (in memory) until acknowledged, so a tab switch doesn't lose them.
		expect(getPendingBackupCodes()).toEqual(['code-1', 'code-2', 'code-3']);
	});

	it('offers to disable 2fa for an enrolled user and disables it', async () => {
		api.disable2FA.mockResolvedValue(undefined);
		const user = userEvent.setup();
		renderPage(sessions, true, '2fa');

		expect(
			screen.queryByRole('button', { name: /enable two-factor auth/i })
		).not.toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /disable 2fa/i }));
		await user.type(
			screen.getAllByLabelText(/current password/i).at(-1) as HTMLElement,
			'ValidPass123!'
		);
		await user.type(screen.getByLabelText(/authenticator code/i), '123456');
		await user.click(screen.getByRole('button', { name: /disable 2fa/i }));

		expect(api.disable2FA).toHaveBeenCalledWith('ValidPass123!', '123456');
		expect(
			await screen.findByRole('button', { name: /enable two-factor auth/i })
		).toBeInTheDocument();
		expect(setAuthenticated).toHaveBeenCalledWith(expect.objectContaining({ totp_enabled: false }));
	});

	it('loads sessions when the sessions tab is opened and revokes other sessions', async () => {
		const user = userEvent.setup();
		renderPage(sessions, false, 'sessions');

		expect(api.listSessions).toHaveBeenCalledOnce();
		expect(await screen.findByText(/sign out other sessions/i)).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /^revoke$/i }));
		expect(api.revokeSession).not.toHaveBeenCalled();
		await confirmDialog(user);

		expect(api.revokeSession).toHaveBeenCalledWith('sess_2');
		expect(screen.queryByText(/10\.0\.0\.2/i)).not.toBeInTheDocument();
	});

	it('revokes all non-current sessions', async () => {
		const user = userEvent.setup();
		renderPage(sessions, false, 'sessions');

		await screen.findByText(/sign out other sessions/i);
		await user.click(screen.getByRole('button', { name: /sign out other sessions/i }));
		await confirmDialog(user);

		expect(api.revokeAllSessions).toHaveBeenCalledOnce();
		expect(screen.queryByText(/10\.0\.0\.2/i)).not.toBeInTheDocument();
		expect(screen.getByText(/this session/i)).toBeInTheDocument();
	});
});
