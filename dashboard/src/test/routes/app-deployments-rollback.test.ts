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
		duration_secs: 60,
		image_pruned: false,
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
		commit_sha: 'cccccccccccccccccccc',
		commit_message: 'Add dark mode',
		trigger_detail: 'ada@example.com',
		is_current: true,
		rollback_unavailable_reason: 'This image is already running'
	}),
	deployment({
		id: 'dep_prev',
		image_tag: 'tond-web:bbbbbbbbbbbb',
		commit_sha: 'bbbbbbbbbbbbbbbbbbbb',
		commit_message: 'Fix login redirect',
		rollback_available: true
	}),
	deployment({
		id: 'dep_failed',
		status: 'failed',
		image_tag: null,
		trigger_source: 'webhook',
		trigger_detail: 'GitHub push to main',
		failure: {
			code: 'build_failed',
			title: 'The build failed (exit code 1)',
			hint: 'Fix it'
		}
	}),
	deployment({
		id: 'dep_pruned',
		image_tag: 'tond-web:aaaaaaaaaaaa',
		image_pruned: true,
		rollback_unavailable_reason: "This deployment's image was pruned to save disk space"
	})
];

function rows() {
	return within(screen.getByRole('list', { name: 'Deployments' })).getAllByRole('listitem');
}

describe('app deployments', () => {
	it('shows commit, trigger, duration and failure reason, and links each row to its log', () => {
		render(AppDeploymentsHarness, { props: { deployments, onRollback: vi.fn() } });

		const [current, prev, failed] = rows();
		expect(within(current).getByText('ccccccc Add dark mode')).toBeInTheDocument();
		expect(within(current).getByText('current')).toBeInTheDocument();
		expect(within(current).getByText(/Manual · ada@example.com/)).toBeInTheDocument();
		expect(within(current).getByText(/1m 00s/)).toBeInTheDocument();
		expect(within(failed).getByText('The build failed (exit code 1)')).toBeInTheDocument();
		expect(within(failed).getByText(/Webhook · GitHub push to main/)).toBeInTheDocument();
		expect(within(prev).getByRole('link')).toHaveAttribute(
			'href',
			'/apps/app_1/deployments/dep_prev'
		);
	});

	it('offers rollback only for deployments whose exact image is still available', () => {
		render(AppDeploymentsHarness, { props: { deployments, onRollback: vi.fn() } });

		const [current, prev, , pruned] = rows();
		expect(
			within(current).queryByRole('button', { name: /roll back to this/i })
		).not.toBeInTheDocument();
		expect(within(prev).getByRole('button', { name: /roll back to this/i })).toBeEnabled();
		expect(within(pruned).getByRole('button', { name: /roll back to this/i })).toBeDisabled();
		expect(within(pruned).getByText('(pruned)')).toBeInTheDocument();
	});

	it('names the target and what stays before rolling back', async () => {
		const user = userEvent.setup();
		const onRollback = vi.fn();
		render(AppDeploymentsHarness, { props: { deployments, onRollback } });

		await user.click(within(rows()[1]).getByRole('button', { name: /roll back to this/i }));
		const dialog = await screen.findByRole('dialog');
		expect(within(dialog).getByText('Roll back web?')).toBeInTheDocument();
		expect(within(dialog).getByText('Fix login redirect')).toBeInTheDocument();
		expect(within(dialog).getByText('tond-web:bbbbbbbbbbbb')).toBeInTheDocument();
		expect(within(dialog).getByText(/environment variables, volumes/i)).toBeInTheDocument();
		expect(onRollback).not.toHaveBeenCalled();
		await user.click(within(dialog).getByRole('button', { name: /^roll back$/i }));

		expect(onRollback).toHaveBeenCalledWith('dep_prev');
	});

	it('disables the roll back button when nothing can be restored', () => {
		render(AppDeploymentsHarness, {
			props: { deployments: [deployments[0], deployments[3]], onRollback: vi.fn() }
		});
		expect(screen.getByRole('button', { name: /roll back to previous/i })).toBeDisabled();
	});

	it('offers no rollback to roles without apps:rollback', () => {
		render(AppDeploymentsHarness, {
			props: { deployments, onRollback: vi.fn(), canRollback: false }
		});
		expect(screen.queryByRole('button', { name: /roll back to this/i })).not.toBeInTheDocument();
		expect(screen.getByRole('button', { name: /roll back to previous/i })).toBeDisabled();
	});
});
