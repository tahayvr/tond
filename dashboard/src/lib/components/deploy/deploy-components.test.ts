import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { Deployment } from '$lib/api';
import DeployResult from './deploy-result.svelte';
import DeploySteps from './deploy-steps.svelte';

function dep(overrides: Partial<Deployment>): Deployment {
	return {
		id: 'd1',
		app_id: 'app_1',
		image_tag: 'tond-web:abc',
		status: 'succeeded',
		trigger_source: 'manual',
		has_generated_dockerfile: false,
		started_at: '2026-01-01T00:00:00Z',
		finished_at: '2026-01-01T00:01:05Z',
		created_at: '2026-01-01T00:00:00Z',
		duration_secs: 65,
		image_pruned: false,
		is_current: true,
		rollback_available: false,
		rollback_unavailable_reason: null,
		...overrides
	};
}

describe('DeployResult', () => {
	it('shows how long a successful deploy took and where the app is', () => {
		render(DeployResult, {
			props: { deployment: dep({}), appId: 'app_1', domain: 'web.example.com' }
		});
		expect(screen.getByText('Deployed in 1m 05s')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'web.example.com' })).toHaveAttribute(
			'href',
			'https://web.example.com/'
		);
	});

	it('shows the likely cause, the lines that show it and where to fix it', async () => {
		const onRedeploy = vi.fn();
		const user = userEvent.setup();
		render(DeployResult, {
			props: {
				deployment: dep({
					status: 'failed',
					is_current: false,
					failure: {
						phase: 'health',
						code: 'port_not_listening',
						title: 'Nothing answered on port 8080 within 60s',
						hint: 'Change the port.',
						excerpt: ['listening on 127.0.0.1:3000']
					}
				}),
				appId: 'app_1',
				onRedeploy
			}
		});
		expect(screen.getByText('Nothing answered on port 8080 within 60s')).toBeInTheDocument();
		expect(screen.getByText('Change the port.')).toBeInTheDocument();
		expect(screen.getByText('listening on 127.0.0.1:3000')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: /port and health settings/i })).toHaveAttribute(
			'href',
			'/apps/app_1?tab=health'
		);
		await user.click(screen.getByRole('button', { name: /deploy again/i }));
		expect(onRedeploy).toHaveBeenCalledOnce();
	});

	it('shows unknown statuses generically', () => {
		render(DeployResult, {
			props: { deployment: dep({ status: 'superseded' }), appId: 'app_1' }
		});
		expect(screen.getByText('Superseded by a newer deploy')).toBeInTheDocument();
	});
});

describe('DeploySteps', () => {
	it('lists visible steps with their state and time', () => {
		render(DeploySteps, {
			props: {
				now: Date.parse('2026-01-01T00:00:20Z'),
				phases: [
					{
						phase: 'clone',
						status: 'succeeded',
						label: 'Pull image',
						started_at: '2026-01-01T00:00:00Z',
						finished_at: '2026-01-01T00:00:04Z'
					},
					{ phase: 'build', status: 'skipped' },
					{
						phase: 'start',
						status: 'running',
						label: 'Start container',
						started_at: '2026-01-01T00:00:05Z'
					},
					{ phase: 'health', status: 'pending', label: 'Health check' }
				]
			}
		});
		const items = screen.getAllByRole('listitem');
		expect(items).toHaveLength(3);
		expect(screen.getByText('Pull image')).toBeInTheDocument();
		expect(screen.getByText('4s')).toBeInTheDocument();
		expect(screen.getByText('15s')).toBeInTheDocument();
		expect(items[1]).toHaveAttribute('aria-current', 'step');
		expect(screen.getByText('Waiting')).toBeInTheDocument();
	});
});
