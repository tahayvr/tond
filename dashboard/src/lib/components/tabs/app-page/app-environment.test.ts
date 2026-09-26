import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppEnv } from '$lib/api';
import AppEnvironment from './app-environment.svelte';

const api = vi.hoisted(() => ({
	getAppEnv: vi.fn(),
	patchAppEnv: vi.fn(),
	patchPreviewEnv: vi.fn(),
	revealAppEnvVar: vi.fn(),
	revealPreviewEnvVar: vi.fn()
}));

const auth = vi.hoisted(() => ({ role: 'admin' }));

vi.mock('$lib/auth.svelte', () => ({
	getUser: () => ({ id: 'usr_1', email: 'a@b.c', role: auth.role, totp_enabled: false }),
	isAdmin: () => auth.role === 'admin'
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const MASK = '••••••••';

function envWith(keys: string[], preview: Partial<AppEnv['preview']> = {}): AppEnv {
	return {
		vars: keys.map((key) => ({ key, masked: MASK })),
		preview: { enabled: false, excluded_keys: [], overrides: [], ...preview }
	};
}

function renderTab(
	props: Partial<{ onRedeploy: () => Promise<void>; previewEnabled: boolean }> = {}
) {
	const onRedeploy = props.onRedeploy ?? vi.fn().mockResolvedValue(undefined);
	render(AppEnvironment, {
		props: {
			appId: 'app_1',
			sourceType: 'git',
			registryCredentials: [],
			registryCredentialId: null,
			onRegistryCredentialChange: vi.fn().mockResolvedValue(undefined),
			previewEnabled: props.previewEnabled ?? false,
			onRedeploy
		}
	});
	return { onRedeploy };
}

describe('AppEnvironment', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		// bits-ui leaves `pointer-events: none` on <body> if a test ends with a dialog open.
		document.body.style.pointerEvents = '';
		auth.role = 'admin';
	});

	it('does not offer reveal or copy to non-admins, but still allows editing', async () => {
		auth.role = 'member';
		api.getAppEnv.mockResolvedValue(envWith(['API_KEY']));

		renderTab();

		expect(await screen.findByText('API_KEY')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Reveal API_KEY' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Copy API_KEY' })).not.toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Edit variable API_KEY' })).toBeInTheDocument();
	});

	it('lists existing variables with masked values instead of starting empty', async () => {
		api.getAppEnv.mockResolvedValue(envWith(['API_KEY', 'DATABASE_URL']));

		renderTab();

		expect(await screen.findByText('DATABASE_URL')).toBeInTheDocument();
		expect(screen.getByText('API_KEY')).toBeInTheDocument();
		expect(screen.getAllByLabelText('hidden value')).toHaveLength(2);
		expect(screen.queryByRole('button', { name: /^save$/i })).not.toBeInTheDocument();
	});

	it('reveals a value only when asked', async () => {
		api.getAppEnv.mockResolvedValue(envWith(['API_KEY']));
		api.revealAppEnvVar.mockResolvedValue('sk_live_123');

		const user = userEvent.setup();
		renderTab();

		await user.click(await screen.findByRole('button', { name: 'Reveal API_KEY' }));

		expect(api.revealAppEnvVar).toHaveBeenCalledWith('app_1', 'API_KEY');
		expect(await screen.findByText('sk_live_123')).toBeInTheDocument();
	});

	it('adds a variable with a patch that touches only that key', async () => {
		api.getAppEnv.mockResolvedValue(envWith(['EXISTING']));
		api.patchAppEnv.mockResolvedValue(envWith(['EXISTING', 'NEW_KEY']));

		const user = userEvent.setup();
		renderTab();

		await user.click(await screen.findByRole('button', { name: /add variable/i }));
		await user.type(screen.getByLabelText('Key'), 'NEW_KEY');
		await user.type(screen.getByLabelText('Value'), 'hello');
		await user.click(screen.getByRole('button', { name: /^save$/i }));

		expect(api.patchAppEnv).toHaveBeenCalledWith('app_1', { set: { NEW_KEY: 'hello' } });
		expect(await screen.findByText(/added new_key/i)).toBeInTheDocument();
		expect(screen.getByText(/until the next deploy/i)).toBeInTheDocument();
	});

	it('deletes one variable after confirmation', async () => {
		api.getAppEnv.mockResolvedValue(envWith(['KEEP', 'DROP']));
		api.patchAppEnv.mockResolvedValue(envWith(['KEEP']));

		const user = userEvent.setup();
		renderTab();

		await user.click(await screen.findByRole('button', { name: 'Delete variable DROP' }));
		const dialog = await screen.findByRole('dialog');
		await user.click(within(dialog).getByRole('button', { name: /^delete$/i }));

		expect(api.patchAppEnv).toHaveBeenCalledWith('app_1', { unset: ['DROP'] });
		await waitFor(() => expect(screen.queryByText('DROP')).not.toBeInTheDocument());
		expect(screen.getByText('KEEP')).toBeInTheDocument();
	});

	it('keeps the current value when editing without typing a new one', async () => {
		api.getAppEnv.mockResolvedValue(envWith(['TOKEN']));

		const user = userEvent.setup();
		renderTab();

		await user.click(await screen.findByRole('button', { name: 'Edit variable TOKEN' }));
		await user.click(screen.getByRole('button', { name: /^save$/i }));

		expect(api.patchAppEnv).not.toHaveBeenCalled();
	});

	it('previews and imports a pasted .env block without removing other variables', async () => {
		api.getAppEnv.mockResolvedValue(envWith(['EXISTING', 'OTHER']));
		api.patchAppEnv.mockResolvedValue(envWith(['EXISTING', 'NEW_ONE', 'OTHER']));

		const user = userEvent.setup();
		renderTab();

		await user.click(await screen.findByRole('button', { name: /import \.env/i }));
		await user.click(screen.getByLabelText('.env contents'));
		await user.paste('# comment\nEXISTING=updated\nNEW_ONE="quoted value"');

		expect(screen.getByText('Add 1')).toBeInTheDocument();
		expect(screen.getByText(/replace the value of 1 existing/i)).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /^import 2$/i }));

		expect(api.patchAppEnv).toHaveBeenCalledWith('app_1', {
			set: { EXISTING: 'updated', NEW_ONE: 'quoted value' }
		});
	});

	it('blocks importing when a pasted line is invalid', async () => {
		api.getAppEnv.mockResolvedValue(envWith([]));

		const user = userEvent.setup();
		renderTab();

		await user.click(await screen.findByRole('button', { name: /import \.env/i }));
		await user.click(screen.getByLabelText('.env contents'));
		await user.paste('GOOD=1\nnot a variable');

		expect(screen.getByText(/line 2/i)).toBeInTheDocument();
		expect(screen.getByRole('button', { name: /^import 1$/i })).toBeDisabled();
	});

	it('saves and redeploys from the dialog', async () => {
		api.getAppEnv.mockResolvedValue(envWith([]));
		api.patchAppEnv.mockResolvedValue(envWith(['PORT']));

		const user = userEvent.setup();
		const { onRedeploy } = renderTab();

		await user.click(await screen.findByRole('button', { name: /add variable/i }));
		await user.type(screen.getByLabelText('Key'), 'PORT');
		await user.type(screen.getByLabelText('Value'), '8080');
		await user.click(screen.getByRole('button', { name: /save and redeploy/i }));

		expect(api.patchAppEnv).toHaveBeenCalledWith('app_1', { set: { PORT: '8080' } });
		await waitFor(() => expect(onRedeploy).toHaveBeenCalledOnce());
		expect(await screen.findByText(/redeploy started/i)).toBeInTheDocument();
	});

	it('shows a retry instead of an empty editor when loading fails', async () => {
		api.getAppEnv.mockRejectedValueOnce(new Error('Server unavailable'));
		api.getAppEnv.mockResolvedValueOnce(envWith(['API_KEY']));

		const user = userEvent.setup();
		renderTab();

		expect(await screen.findByText(/could not load environment variables/i)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: /add variable/i })).not.toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /retry/i }));
		expect(await screen.findByText('API_KEY')).toBeInTheDocument();
	});

	it('toggles preview exclusion with a preview-only patch', async () => {
		api.getAppEnv.mockResolvedValue(envWith(['SECRET'], { enabled: true }));
		api.patchPreviewEnv.mockResolvedValue(
			envWith(['SECRET'], { enabled: true, excluded_keys: ['SECRET'] })
		);

		const user = userEvent.setup();
		renderTab({ previewEnabled: true });

		await user.click(await screen.findByLabelText('Exclude SECRET from previews'));

		expect(api.patchPreviewEnv).toHaveBeenCalledWith('app_1', { exclude: ['SECRET'] });
		expect(api.patchAppEnv).not.toHaveBeenCalled();
	});
});
