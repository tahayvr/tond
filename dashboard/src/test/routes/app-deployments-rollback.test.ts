import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import type { Deployment } from '$lib/api';
import AppDeploymentsHarness from '../mocks/AppDeploymentsHarness.svelte';

function deployment(overrides: Partial<Deployment>): Deployment {
	return {
		id: 'dep',
		app_id: 'app_1',
		image_tag: 'tond-web:aaaaaaaaaaaa',
		status: 'succeeded',
		trigger_source: 'manual',
		has_generated_dockerfile: false,
		started_at: '2026-01-01T00:00:00.000Z',
		finished_at: '2026-01-01T00:01:00.000Z',
		created_at: '2026-01-01T00:00:00.000Z',
		image_pruned: false,
		kind: 'deploy',
		error: null,
		superseded_by: null,
		queue_position: null,
		is_current: false,
		rollback_available: false,
		rollback_unavailable_reason: null,
		...overrides
	};
}

const deployments: Deployment[] = [
	deployment({
		id: 'dep_current',
		image_tag: 'tond-web:cccccccccccc',
		is_current: true,
		rollback_unavailable_reason: 'This image is already running'
	}),
	deployment({ id: 'dep_prev', image_tag: 'tond-web:bbbbbbbbbbbb', rollback_available: true }),
	deployment({
		id: 'dep_pruned',
		image_tag: 'tond-web:aaaaaaaaaaaa',
		image_pruned: true,
		rollback_unavailable_reason: "This deployment's image was pruned to save disk space"
	})
];

describe('app deployments rollback', () => {
	it('offers restore only for deployments whose exact image is still available', () => {
		render(AppDeploymentsHarness, { props: { deployments, onRollback: vi.fn() } });

		const rows = screen.getAllByRole('row').slice(1);
		expect(within(rows[0]).getByText('current')).toBeInTheDocument();
		expect(within(rows[0]).queryByRole('button', { name: /restore/i })).not.toBeInTheDocument();
		expect(within(rows[1]).getByRole('button', { name: /restore/i })).toBeEnabled();
		expect(within(rows[2]).getByRole('button', { name: /restore/i })).toBeDisabled();
		expect(within(rows[2]).getByText('(pruned)')).toBeInTheDocument();
	});

	it('restores the chosen deployment after confirmation', async () => {
		const user = userEvent.setup();
		const onRollback = vi.fn();
		render(AppDeploymentsHarness, { props: { deployments, onRollback } });

		const rows = screen.getAllByRole('row').slice(1);
		await user.click(within(rows[1]).getByRole('button', { name: /restore/i }));
		expect(await screen.findByText(/restore this deployment\?/i)).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /restore image/i }));

		expect(onRollback).toHaveBeenCalledWith('dep_prev');
	});

	it('shows queue position, progress and why a deploy was superseded or failed', () => {
		render(AppDeploymentsHarness, {
			props: {
				deployments: [
					deployment({ id: 'dep_q', status: 'queued', queue_position: 2, image_tag: null }),
					deployment({ id: 'dep_b', status: 'building', image_tag: null }),
					deployment({
						id: 'dep_s',
						status: 'superseded',
						superseded_by: 'dep_q',
						image_tag: null
					}),
					deployment({
						id: 'dep_f',
						status: 'failed',
						error: 'Interrupted: Tond restarted while this was in progress. Deploy again to retry.'
					})
				],
				onRollback: vi.fn()
			}
		});

		const rows = screen.getAllByRole('row').slice(1);
		expect(within(rows[0]).getByText('Queued')).toBeInTheDocument();
		expect(within(rows[0]).getByText('2nd in line')).toBeInTheDocument();
		expect(within(rows[0]).getByText('Waiting')).toBeInTheDocument();
		expect(within(rows[1]).getByText('Building')).toBeInTheDocument();
		expect(within(rows[2]).getByText('Superseded')).toBeInTheDocument();
		expect(
			within(rows[2]).getByText('Replaced by a newer deploy before it started')
		).toBeInTheDocument();
		expect(within(rows[3]).getByText(/Deploy again to retry/)).toBeInTheDocument();
	});

	it('disables the roll back button when nothing can be restored', () => {
		render(AppDeploymentsHarness, {
			props: { deployments: [deployments[0], deployments[2]], onRollback: vi.fn() }
		});
		expect(screen.getByRole('button', { name: /roll back to previous/i })).toBeDisabled();
	});
});
