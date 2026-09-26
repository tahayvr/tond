import { render, screen } from '@testing-library/svelte';
import { flushSync } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { App, DeploymentDetail, DeploymentStreamCallbacks } from '$lib/api';

const { goto, setUiContext, clearUiContext, appContext } = vi.hoisted(() => ({
	goto: vi.fn(),
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	appContext: vi.fn((app) => ({
		type: 'app',
		app,
		feed: { title: app.name, wsUrl: '/api/apps/x/logs/stream' }
	}))
}));

const stream = vi.hoisted(() => ({
	callbacks: null as DeploymentStreamCallbacks | null,
	close: vi.fn()
}));

const api = vi.hoisted(() => ({
	deployApp: vi.fn(),
	getDeployment: vi.fn(),
	openDeploymentStream: vi.fn()
}));

vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$lib/ui-context.svelte', () => ({ setUiContext, clearUiContext, appContext }));
vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

import DeployPage from '../../routes/apps/[id]/deployments/[deploymentId]/+page.svelte';

const app = { id: 'app_1', name: 'web', domain: 'web.example.com' } as App;

function detail(overrides: Partial<DeploymentDetail> = {}): DeploymentDetail {
	return {
		id: 'dep_live_123',
		app_id: 'app_1',
		image_tag: null,
		status: 'running',
		trigger_source: 'manual',
		trigger_detail: 'ada@example.com',
		has_generated_dockerfile: false,
		started_at: new Date(Date.now() - 5000).toISOString(),
		finished_at: null,
		created_at: new Date().toISOString(),
		phases: [
			{
				phase: 'clone',
				status: 'running',
				label: 'Clone repository',
				started_at: new Date().toISOString()
			},
			{ phase: 'build', status: 'pending', label: 'Build image' },
			{ phase: 'start', status: 'pending', label: 'Start container' },
			{ phase: 'health', status: 'pending', label: 'Health check' },
			{ phase: 'release', status: 'pending', label: 'Switch traffic' }
		],
		failure: null,
		image_pruned: false,
		is_current: false,
		rollback_available: false,
		rollback_unavailable_reason: null,
		logs: [{ level: 'info', message: 'Starting deployment...', ts: new Date().toISOString() }],
		logs_truncated: false,
		container_port: 8080,
		health_check_path: '/healthz',
		source_type: 'git',
		...overrides
	};
}

describe('live deploy view', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		setUiContext.mockReset();
		stream.callbacks = null;
		stream.close.mockReset();
		api.openDeploymentStream.mockImplementation(
			(_app: string, _dep: string, callbacks: DeploymentStreamCallbacks) => {
				stream.callbacks = callbacks;
				return stream.close;
			}
		);
	});

	it('streams steps and log lines, then shows the result', async () => {
		const d = detail();
		render(DeployPage, { props: { data: { app, deployment: d }, params: {} } as never });

		expect(screen.getByRole('heading', { name: /deploying web/i })).toBeInTheDocument();
		expect(screen.getByText(/manual · ada@example.com/i)).toBeInTheDocument();
		expect(api.openDeploymentStream).toHaveBeenCalledWith(
			'app_1',
			'dep_live_123',
			expect.anything()
		);
		// The feed panel's own stream is paused on this page.
		expect(setUiContext).toHaveBeenCalledWith(
			expect.objectContaining({ feed: expect.objectContaining({ wsUrl: null }) })
		);

		stream.callbacks?.onLog({
			level: 'info',
			message: 'Cloning repo',
			ts: new Date().toISOString()
		});
		stream.callbacks?.onDeployment({
			...d,
			commit_sha: 'abcdef1234567',
			commit_message: 'Fix login'
		});
		expect(await screen.findByText('Cloning repo')).toBeInTheDocument();
		expect(screen.getByText('Fix login')).toBeInTheDocument();

		stream.callbacks?.onDeployment({
			...d,
			status: 'failed',
			finished_at: new Date().toISOString(),
			duration_secs: 12,
			failure: {
				phase: 'build',
				code: 'build_failed',
				title: 'The build failed (exit code 1)',
				hint: 'Fix the error.'
			}
		});
		flushSync();
		expect(await screen.findByText('The build failed (exit code 1)')).toBeInTheDocument();
		expect(screen.getByRole('heading', { name: /deployment of web/i })).toBeInTheDocument();
	});

	it('does not open a stream for a finished deployment', () => {
		render(DeployPage, {
			props: {
				data: {
					app,
					deployment: detail({
						status: 'succeeded',
						finished_at: new Date().toISOString(),
						duration_secs: 30
					})
				},
				params: {}
			} as never
		});
		expect(api.openDeploymentStream).not.toHaveBeenCalled();
		expect(screen.getByText('Deployed in 30s')).toBeInTheDocument();
	});

	it('shows a queued deployment gracefully', () => {
		render(DeployPage, {
			props: {
				data: { app, deployment: detail({ status: 'queued', queue_position: 2 }) },
				params: {}
			} as never
		});
		expect(screen.getByText(/waiting for the build queue \(position 2\)/i)).toBeInTheDocument();
	});

	it('replaces what it shows with the snapshot after a reconnect', async () => {
		render(DeployPage, { props: { data: { app, deployment: detail() }, params: {} } as never });
		stream.callbacks?.onOpen?.(false);
		expect(await screen.findByText('Live')).toBeInTheDocument();
		stream.callbacks?.onDisconnect?.(1000);
		await vi.waitFor(() => expect(screen.queryByText('Live')).not.toBeInTheDocument());
		stream.callbacks?.onSnapshot(
			detail({
				status: 'succeeded',
				finished_at: new Date().toISOString(),
				duration_secs: 9,
				is_current: true,
				logs: [{ level: 'info', message: 'Deployed successfully!', ts: new Date().toISOString() }]
			})
		);
		expect(await screen.findByText('Deployed in 9s')).toBeInTheDocument();
		expect(await screen.findByText('Deployed successfully!')).toBeInTheDocument();
	});
});
