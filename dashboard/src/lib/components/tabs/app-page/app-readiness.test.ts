import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { App } from '$lib/api';
import AppReadiness from './app-readiness.svelte';

const api = vi.hoisted(() => ({ updateReadiness: vi.fn() }));
const confirm = vi.hoisted(() => ({ confirmAction: vi.fn() }));

vi.mock('$lib/confirm.svelte', () => confirm);

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const app = {
	id: 'app_1',
	name: 'jobs',
	domain: 'jobs.example.com',
	source_type: 'image',
	readiness: 'http',
	ready_after_secs: 10
} as App;

describe('AppReadiness', () => {
	beforeEach(() => {
		api.updateReadiness.mockReset();
		confirm.confirmAction.mockReset();
		confirm.confirmAction.mockResolvedValue(true);
		api.updateReadiness.mockImplementation(async (_id, readiness, secs) => ({
			...app,
			readiness,
			ready_after_secs: secs ?? 10,
			domain: readiness === 'none' ? null : app.domain
		}));
	});

	it('asks before making an app with a domain a background worker', async () => {
		const onSaved = vi.fn();
		const user = userEvent.setup();
		render(AppReadiness, { props: { app, onSaved } });

		await user.click(screen.getByRole('radio', { name: /background worker/i }));
		const secs = screen.getByLabelText(/stays up for/i);
		await user.clear(secs);
		await user.type(secs, '20');
		await user.click(screen.getByRole('button', { name: /^save$/i }));

		await vi.waitFor(() => expect(api.updateReadiness).toHaveBeenCalledWith('app_1', 'none', 20));
		const asked = confirm.confirmAction.mock.calls[0][0];
		expect(asked.confirmLabel).toBe('Make it a worker');
		expect(asked.details.join(' ')).toContain('jobs.example.com');
		expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({ readiness: 'none' }));
	});

	it('keeps the app as it is when the confirmation is cancelled', async () => {
		confirm.confirmAction.mockResolvedValue(false);
		const user = userEvent.setup();
		render(AppReadiness, { props: { app, onSaved: vi.fn() } });
		await user.click(screen.getByRole('radio', { name: /background worker/i }));
		await user.click(screen.getByRole('button', { name: /^save$/i }));
		await vi.waitFor(() => expect(confirm.confirmAction).toHaveBeenCalled());
		expect(api.updateReadiness).not.toHaveBeenCalled();
	});

	it('saves a TCP service without confirmation', async () => {
		const user = userEvent.setup();
		render(AppReadiness, { props: { app, onSaved: vi.fn() } });
		await user.click(screen.getByRole('radio', { name: /tcp service/i }));
		await user.click(screen.getByRole('button', { name: /^save$/i }));
		expect(api.updateReadiness).toHaveBeenCalledWith('app_1', 'tcp', undefined);
		expect(confirm.confirmAction).not.toHaveBeenCalled();
	});

	it('is read-only without apps:write', () => {
		render(AppReadiness, { props: { app, onSaved: vi.fn(), canEdit: false } });
		expect(screen.getByRole('radio', { name: /tcp service/i })).toBeDisabled();
		expect(screen.getByText(/your role can't change/i)).toBeInTheDocument();
	});
});
