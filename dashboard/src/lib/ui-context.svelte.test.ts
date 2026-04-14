import { beforeEach, describe, expect, it } from 'vitest';

import {
	appContext,
	clearUiContext,
	databasesContext,
	getFeedOpen,
	getPreferredTerminalTarget,
	getTerminalState,
	getUiContext,
	openFeedPanel,
	projectsContext,
	setFeedOpen,
	setTerminalOpen,
	setTerminalTarget,
	setUiContext,
	syncTerminalTarget,
	toggleTerminalOpen
} from './ui-context.svelte';

const app = {
	id: 'app_running',
	name: 'api-service',
	domain: 'api.example.com',
	source_type: 'git',
	source_url: 'https://github.com/example/api-service',
	status: 'running',
	container_port: 8080,
	host_port: 32001,
	memory_limit: null,
	cpu_limit: null,
	health_check_path: '/healthz',
	health_check_interval: 30,
	health_failure_threshold: 3,
	health_consecutive_failures: 0,
	volumes: [],
	drain_timeout: 30,
	build_method: 'tbuild',
	auto_deploy: false,
	watch_branch: 'main',
	deploy_key_pub: null,
	registry_credential_id: null,
	github_app_id: null,
	project_id: null,
	dockerfile_content: null,
	compose_content: null,
	base_dir: null,
	static_build: false,
	preview_enabled: false,
	preview_excluded_keys: [],
	preview_env_vars: {},
	created_at: '2026-01-01T00:00:00.000Z',
	updated_at: '2026-01-01T00:00:00.000Z'
};

describe('ui context state', () => {
	beforeEach(() => {
		clearUiContext();
		setFeedOpen(false);
		setTerminalOpen(false);
		setTerminalTarget(null);
	});

	it('builds the expected app context feed descriptor', () => {
		const context = appContext(app);

		expect(context).toMatchObject({
			type: 'app',
			app: { id: 'app_running' },
			feed: {
				title: 'api-service',
				wsUrl: '/api/apps/app_running/logs/stream',
				label: 'live stream'
			}
		});
	});

	it('opens the feed panel imperatively', () => {
		setFeedOpen(false);

		openFeedPanel();

		expect(getFeedOpen()).toBe(true);
	});

	it('closes the feed when the terminal opens and resets manual selection on close', () => {
		setFeedOpen(true);
		setTerminalTarget({ id: 'app_running', kind: 'app' });

		setTerminalOpen(true);

		expect(getFeedOpen()).toBe(false);
		expect(getTerminalState()).toMatchObject({
			open: true,
			selectedTargetId: 'app_running',
			selectedTargetKind: 'app',
			manualSelection: true
		});

		setTerminalOpen(false);

		expect(getTerminalState()).toMatchObject({
			open: false,
			manualSelection: false
		});
	});

	it('prefers the current app terminal target when available', () => {
		setUiContext(appContext(app));

		const preferred = getPreferredTerminalTarget([
			{ id: 'server', kind: 'server', label: 'Server', shell_user: 'tond' },
			{ id: 'app_running', kind: 'app', label: 'App: api-service', shell_user: null }
		]);

		expect(preferred).toMatchObject({ id: 'app_running', kind: 'app' });
	});

	it('falls back to the server target when no context-specific target exists', () => {
		setUiContext(databasesContext());

		const preferred = getPreferredTerminalTarget([
			{ id: 'server', kind: 'server', label: 'Server', shell_user: 'tond' },
			{ id: 'db_1', kind: 'database', label: 'Database: app-postgres', shell_user: null }
		]);

		expect(preferred).toMatchObject({ id: 'server', kind: 'server' });
	});

	it('keeps the active manual terminal target when it still exists', () => {
		setUiContext(projectsContext());
		setTerminalTarget({ id: 'db_1', kind: 'database' });

		const active = syncTerminalTarget([
			{ id: 'server', kind: 'server', label: 'Server', shell_user: 'tond' },
			{ id: 'db_1', kind: 'database', label: 'Database: app-postgres', shell_user: null }
		]);

		expect(active).toMatchObject({ id: 'db_1', kind: 'database' });
		expect(getTerminalState()).toMatchObject({
			selectedTargetId: 'db_1',
			selectedTargetKind: 'database',
			manualSelection: true
		});
	});

	it('syncs back to the preferred target when the manual target disappears', () => {
		setUiContext(appContext(app));
		setTerminalTarget({ id: 'missing-db', kind: 'database' });

		const synced = syncTerminalTarget([
			{ id: 'server', kind: 'server', label: 'Server', shell_user: 'tond' },
			{ id: 'app_running', kind: 'app', label: 'App: api-service', shell_user: null }
		]);

		expect(synced).toMatchObject({ id: 'app_running', kind: 'app' });
		expect(getTerminalState()).toMatchObject({
			selectedTargetId: 'app_running',
			selectedTargetKind: 'app',
			manualSelection: true
		});
	});

	it('can toggle the terminal open state', () => {
		setTerminalOpen(false);

		toggleTerminalOpen();
		expect(getTerminalState().open).toBe(true);

		toggleTerminalOpen();
		expect(getTerminalState().open).toBe(false);
	});

	it('clears back to the idle ui context', () => {
		setUiContext(appContext(app));

		clearUiContext();

		expect(getUiContext()).toEqual({
			type: 'idle',
			feed: { title: 'Feed', wsUrl: null }
		});
	});
});
