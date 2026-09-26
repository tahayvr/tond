import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { confirmDialog, renderConfirmHost } from '../../../test/confirm';
import type { EmailConfig } from '$lib/api';
import NotificationChannelList from './notification-channel-list.svelte';

const api = vi.hoisted(() => ({
	listIntegrations: vi.fn(),
	createIntegration: vi.fn(),
	updateIntegration: vi.fn(),
	deleteIntegration: vi.fn(),
	testIntegration: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return {
		...actual,
		...api
	};
});

describe('NotificationChannelList', () => {
	const emailConfig: EmailConfig = {
		from_name: 'Tond',
		from_addr: 'alerts@example.com',
		smtp_host: 'smtp.example.com',
		smtp_port: 587,
		smtp_user: 'alerts@example.com',
		smtp_encryption: 'starttls',
		to_addrs: ['ops@example.com']
	};

	beforeEach(() => {
		for (const fn of Object.values(api)) {
			fn.mockReset();
		}
		api.createIntegration.mockImplementation(async ({ name, kind, config }) => ({
			id: 'int_new',
			name,
			kind,
			config,
			saved_secrets: ['smtp_password'],
			enabled: true,
			created_at: '2026-01-02T00:00:00.000Z',
			updated_at: '2026-01-02T00:00:00.000Z'
		}));
		api.updateIntegration.mockImplementation(async (id, payload) => ({
			id,
			name: payload.name ?? 'Ops Email',
			kind: 'email',
			config: payload.config ?? emailConfig,
			saved_secrets: ['smtp_password'],
			enabled: payload.enabled ?? true,
			created_at: '2026-01-01T00:00:00.000Z',
			updated_at: '2026-01-03T00:00:00.000Z'
		}));
		api.deleteIntegration.mockResolvedValue(undefined);
		api.testIntegration.mockResolvedValue({ message: 'Test notification sent.' });
	});

	it('renders route-provided integrations without loading on mount', () => {
		render(NotificationChannelList, {
			props: {
				kind: 'email',
				label: 'Email',
				initialIntegrations: [
					{
						id: 'int_1',
						kind: 'email',
						name: 'Ops Email',
						enabled: true,
						config: {
							from_name: 'Tond',
							from_addr: 'alerts@example.com',
							smtp_host: 'smtp.example.com',
							smtp_port: 587,
							smtp_user: 'alerts@example.com',
							smtp_encryption: 'starttls',
							to_addrs: ['ops@example.com']
						},
						saved_secrets: ['smtp_password'],
						created_at: '2026-01-01T00:00:00.000Z',
						updated_at: '2026-01-01T00:00:00.000Z'
					}
				],
				buildConfig: () => emailConfig,
				resetConfig: vi.fn(),
				loadConfig: vi.fn()
			}
		});

		expect(screen.queryByText(/loading/i)).not.toBeInTheDocument();
		expect(screen.getByText(/ops email/i)).toBeInTheDocument();
		expect(api.listIntegrations).not.toHaveBeenCalled();
	});

	it('adds a new channel', async () => {
		const user = userEvent.setup();
		render(NotificationChannelList, {
			props: {
				kind: 'email',
				label: 'Email',
				initialIntegrations: [],
				buildConfig: () => emailConfig,
				resetConfig: vi.fn(),
				loadConfig: vi.fn()
			}
		});

		await user.click(screen.getByRole('button', { name: /add channel/i }));
		await user.type(screen.getByLabelText(/display name/i), 'Pager Email');
		await user.click(screen.getByRole('button', { name: /save channel/i }));

		expect(api.createIntegration).toHaveBeenCalledWith({
			name: 'Pager Email',
			kind: 'email',
			config: emailConfig
		});
		expect(await screen.findByText(/pager email/i)).toBeInTheDocument();
	});

	it('toggles and tests an existing channel', async () => {
		const user = userEvent.setup();
		render(NotificationChannelList, {
			props: {
				kind: 'email',
				label: 'Email',
				initialIntegrations: [
					{
						id: 'int_1',
						kind: 'email',
						name: 'Ops Email',
						enabled: true,
						config: emailConfig,
						saved_secrets: ['smtp_password'],
						created_at: '2026-01-01T00:00:00.000Z',
						updated_at: '2026-01-01T00:00:00.000Z'
					}
				],
				buildConfig: () => emailConfig,
				resetConfig: vi.fn(),
				loadConfig: vi.fn()
			}
		});

		await user.click(screen.getByRole('button', { name: /^disable$/i }));
		expect(api.updateIntegration).toHaveBeenCalledWith('int_1', { enabled: false });

		await user.click(screen.getByRole('button', { name: /^test$/i }));
		expect(api.testIntegration).toHaveBeenCalledWith('int_1');
		expect(await screen.findByText(/test notification sent/i)).toBeInTheDocument();
	});

	it('deletes a channel', async () => {
		const user = userEvent.setup();
		render(NotificationChannelList, {
			props: {
				kind: 'email',
				label: 'Email',
				initialIntegrations: [
					{
						id: 'int_1',
						kind: 'email',
						name: 'Ops Email',
						enabled: true,
						config: emailConfig,
						saved_secrets: ['smtp_password'],
						created_at: '2026-01-01T00:00:00.000Z',
						updated_at: '2026-01-01T00:00:00.000Z'
					}
				],
				buildConfig: () => emailConfig,
				resetConfig: vi.fn(),
				loadConfig: vi.fn()
			}
		});

		renderConfirmHost();
		await user.click(screen.getByRole('button', { name: /delete ops email/i }));
		await confirmDialog(user);

		expect(api.deleteIntegration).toHaveBeenCalledWith('int_1');
		expect(screen.queryByText(/ops email/i)).not.toBeInTheDocument();
	});
});
