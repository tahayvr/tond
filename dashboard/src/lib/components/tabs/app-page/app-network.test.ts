import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppEgress } from '$lib/api';
import AppNetwork from './app-network.svelte';

const api = vi.hoisted(() => ({ getAppEgress: vi.fn(), updateAppEgress: vi.fn() }));
const confirm = vi.hoisted(() => ({ confirmAction: vi.fn() }));

vi.mock('$lib/confirm.svelte', () => confirm);

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const open: AppEgress = {
	policy: 'open',
	allowlist: [],
	preview_policy: 'none',
	firewall_available: true,
	warning: null,
	preview_warning: null,
	hosts: []
};

function renderNetwork(props: Partial<{ canEdit: boolean; sourceType: string }> = {}) {
	return render(AppNetwork, {
		props: {
			appId: 'app_1',
			appName: 'web',
			sourceType: props.sourceType ?? 'git',
			canEdit: props.canEdit ?? true
		}
	});
}

describe('AppNetwork', () => {
	beforeEach(() => {
		api.getAppEgress.mockReset();
		api.updateAppEgress.mockReset();
		confirm.confirmAction.mockReset();
		confirm.confirmAction.mockResolvedValue(true);
		api.getAppEgress.mockResolvedValue(open);
		api.updateAppEgress.mockImplementation(async (_id, body) => ({
			...open,
			policy: body.policy,
			allowlist: body.allowlist ?? [],
			preview_policy: body.preview_policy ?? 'none',
			hosts: (body.allowlist ?? [])
				.filter((e: string) => /[a-z]/.test(e))
				.map((e: string) => ({ host: e.split(':')[0], addresses: ['192.0.2.10'] }))
		}));
	});

	it('restricts the app to an allowlist after confirming', async () => {
		const user = userEvent.setup();
		renderNetwork();
		const restricted = await screen.findByRole('radio', { name: /Allowlist only/ });
		expect(screen.getByRole('radio', { name: /Open/ })).toHaveAttribute('aria-checked', 'true');
		await user.click(restricted);

		const save = screen.getByRole('button', { name: 'Save' });
		expect(save).toBeDisabled();
		await user.type(screen.getByLabelText('Allowed destinations'), 'api.stripe.com:443');
		await user.click(save);

		expect(confirm.confirmAction).toHaveBeenCalledWith(
			expect.objectContaining({ title: "Restrict web's outbound connections?" })
		);
		expect(api.updateAppEgress).toHaveBeenCalledWith('app_1', {
			policy: 'restricted',
			allowlist: ['api.stripe.com:443'],
			preview_policy: 'none'
		});
		expect(await screen.findByText(/can reach one destination/)).toBeInTheDocument();
		expect(screen.getByText('api.stripe.com')).toBeInTheDocument();
		expect(screen.getByText(/192\.0\.2\.10/)).toBeInTheDocument();
	});

	it('does nothing when the restriction is not confirmed', async () => {
		const user = userEvent.setup();
		confirm.confirmAction.mockResolvedValue(false);
		renderNetwork();
		await user.click(await screen.findByRole('radio', { name: /No internet/ }));
		await user.click(screen.getByRole('button', { name: 'Save' }));
		expect(api.updateAppEgress).not.toHaveBeenCalled();
	});

	it('shows server errors next to the form', async () => {
		const user = userEvent.setup();
		api.getAppEgress.mockResolvedValue({ ...open, policy: 'none' });
		api.updateAppEgress.mockRejectedValue(
			new Error("'*.example.com': wildcards can't be resolved")
		);
		renderNetwork();
		await user.click(await screen.findByRole('radio', { name: /Allowlist only/ }));
		await user.type(screen.getByLabelText('Allowed destinations'), '*.example.com');
		await user.click(screen.getByRole('button', { name: 'Save' }));
		expect(confirm.confirmAction).not.toHaveBeenCalled();
		expect(await screen.findByRole('alert')).toHaveTextContent(/wildcards/);
	});

	it('warns when the server cannot enforce a policy', async () => {
		api.getAppEgress.mockResolvedValue({
			...open,
			firewall_available: false,
			preview_warning: 'Not enforced on this server (iptables isn’t installed)'
		});
		renderNetwork();
		expect(await screen.findByText(/can't enforce a policy/)).toBeInTheDocument();
		expect(screen.getByRole('radio', { name: /No internet/ })).toBeDisabled();
		expect(screen.getByText(/Not enforced on this server/)).toBeInTheDocument();
	});

	it('is read-only without settings:write and hides previews for compose apps', async () => {
		renderNetwork({ canEdit: false, sourceType: 'compose' });
		await waitFor(() => expect(api.getAppEgress).toHaveBeenCalledWith('app_1'));
		expect(await screen.findByText(/Only admins can change/)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
		expect(screen.getByRole('radio', { name: /No internet/ })).toBeDisabled();
		expect(screen.queryByLabelText('Previews')).not.toBeInTheDocument();
	});
});
