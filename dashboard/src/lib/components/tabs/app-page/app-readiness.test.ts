import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { App } from '$lib/api';
import AppReadiness from './app-readiness.svelte';

const api = vi.hoisted(() => ({ updateReadiness: vi.fn() }));

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

		const dialog = await screen.findByRole('dialog');
		expect(within(dialog).getByText('jobs.example.com')).toBeInTheDocument();
		expect(api.updateReadiness).not.toHaveBeenCalled();
		await user.click(within(dialog).getByRole('button', { name: /make it a worker/i }));
		expect(api.updateReadiness).toHaveBeenCalledWith('app_1', 'none', 20);
		expect(onSaved).toHaveBeenCalledWith(expect.objectContaining({ readiness: 'none' }));
	});

	it('saves a TCP service without confirmation', async () => {
		// A dialog from the previous test can leave pointer-events on <body>.
		const user = userEvent.setup({ pointerEventsCheck: 0 });
		render(AppReadiness, { props: { app, onSaved: vi.fn() } });
		await user.click(screen.getByRole('radio', { name: /tcp service/i }));
		await user.click(screen.getByRole('button', { name: /^save$/i }));
		expect(api.updateReadiness).toHaveBeenCalledWith('app_1', 'tcp', undefined);
		expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
	});

	it('is read-only without apps:write', () => {
		render(AppReadiness, { props: { app, onSaved: vi.fn(), canEdit: false } });
		expect(screen.getByRole('radio', { name: /tcp service/i })).toBeDisabled();
		expect(screen.getByText(/your role can't change/i)).toBeInTheDocument();
	});
});
