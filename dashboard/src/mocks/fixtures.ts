/**
 * Shared fixture data used by MSW mock handlers.
 * All IDs are stable so navigation between pages works (e.g. /apps/:id links).
 */

import type {
	App,
	AlertRule,
	AuditLogEntry,
	BackupConfig,
	Certificate,
	CronJob,
	CronJobRun,
	Deployment,
	DeploymentLogLine,
	DeployPhase,
	PhaseRecord,
	DeployKey,
	GitHubApp,
	GitHubRepo,
	GitHubBranch,
	Integration,
	LoopStats,
	ManagedDatabase,
	TemplateCatalogEntry,
	MetricsRange,
	MetricsResponse,
	RequestMetricsResponse,
	RequestRange,
	TrafficOverview,
	OnboardingStatus,
	PreviewDeployment,
	Project,
	ProjectPreview,
	RegistryCredential,
	ServerSettings,
	Snapshot,
	StorageProvider,
	SystemMetrics,
	SystemSelf,
	User,
	AdminUser,
	ApiToken,
	TokenCatalog
} from '$lib/api';
import { ALL_SCOPES } from '$lib/access';

// ── Users ──────────────────────────────────────────────────────────────────

export const MOCK_USER: User = {
	id: 'usr_dev',
	email: 'admin@localhost',
	role: 'admin',
	totp_enabled: false,
	created_at: '2025-01-01T00:00:00.000Z',
	disabled: false,
	password_set: true,
	last_login_at: new Date(Date.now() - 20 * 60_000).toISOString(),
	scopes: ALL_SCOPES
};

const DEPLOYER_SCOPES = [
	'apps:read',
	'apps:deploy',
	'apps:control',
	'apps:rollback',
	'env:read-names',
	'env:write',
	'logs:read',
	'metrics:read',
	'events:read',
	'cron:read',
	'cron:trigger',
	'previews:write',
	'db:read',
	'snapshots:read',
	'snapshots:create'
];
const VIEWER_SCOPES = [
	'apps:read',
	'env:read-names',
	'db:read',
	'snapshots:read',
	'logs:read',
	'metrics:read',
	'events:read',
	'cron:read'
];

export const MOCK_USERS: AdminUser[] = [
	{ ...MOCK_USER, active_sessions: 2, is_you: true },
	{
		id: 'usr_2',
		email: 'dana@example.com',
		role: 'deployer',
		totp_enabled: true,
		created_at: '2025-02-01T00:00:00.000Z',
		disabled: false,
		password_set: true,
		last_login_at: new Date(Date.now() - 3 * 3600_000).toISOString(),
		scopes: DEPLOYER_SCOPES,
		active_sessions: 1,
		is_you: false
	},
	{
		id: 'usr_3',
		email: 'victor@example.com',
		role: 'viewer',
		totp_enabled: false,
		created_at: '2025-03-10T00:00:00.000Z',
		disabled: false,
		password_set: false,
		last_login_at: null,
		scopes: VIEWER_SCOPES,
		active_sessions: 0,
		is_you: false
	},
	{
		id: 'usr_4',
		email: 'former@example.com',
		role: 'deployer',
		totp_enabled: false,
		created_at: '2024-11-01T00:00:00.000Z',
		disabled: true,
		password_set: true,
		last_login_at: '2025-01-20T10:00:00.000Z',
		scopes: DEPLOYER_SCOPES,
		active_sessions: 0,
		is_you: false
	}
];

// ── API tokens ─────────────────────────────────────────────────────────────

export const MOCK_TOKEN_CATALOG: TokenCatalog = {
	groups: [
		{
			id: 'apps',
			label: 'Apps',
			scopes: [
				{
					id: 'apps:read',
					description: 'See apps, their settings, domains and deployment history'
				},
				{ id: 'apps:deploy', description: 'Deploy apps (build and release a new version)' },
				{
					id: 'apps:write',
					description: 'Create apps and change their configuration, domains and volumes'
				},
				{ id: 'apps:control', description: 'Start, stop and restart apps' },
				{ id: 'apps:rollback', description: 'Roll an app back to an earlier deployment' },
				{ id: 'apps:delete', description: 'Delete apps and projects' }
			]
		},
		{
			id: 'secrets',
			label: 'Secrets',
			scopes: [
				{
					id: 'env:read-names',
					description: 'See the names of environment variables (not their values)'
				},
				{ id: 'env:read-values', description: 'Reveal environment variable values' },
				{ id: 'env:write', description: 'Add, change and remove environment variables' },
				{
					id: 'db:credentials',
					description: 'Reveal and change database passwords and connection strings'
				}
			]
		},
		{
			id: 'data',
			label: 'Data',
			scopes: [
				{ id: 'db:read', description: 'See databases and their status' },
				{ id: 'db:write', description: 'Create, start, stop and reconfigure databases' },
				{ id: 'db:delete', description: 'Delete databases' },
				{ id: 'snapshots:read', description: 'See snapshots and backup status' },
				{ id: 'snapshots:create', description: 'Take snapshots' },
				{
					id: 'snapshots:restore',
					description: 'Restore and delete snapshots (overwrites live data)'
				}
			]
		},
		{
			id: 'ops',
			label: 'Ops',
			scopes: [
				{ id: 'logs:read', description: 'Read build, app, database and cron logs' },
				{ id: 'metrics:read', description: 'See CPU, memory and network metrics and alert rules' },
				{ id: 'events:read', description: 'See the activity (audit) log' },
				{ id: 'cron:read', description: 'See cron jobs and their runs' },
				{ id: 'cron:write', description: 'Create, change and delete cron jobs' },
				{ id: 'cron:trigger', description: 'Run a cron job now' },
				{ id: 'previews:write', description: 'Create and delete preview deployments' }
			]
		},
		{
			id: 'admin',
			label: 'Admin',
			scopes: [
				{
					id: 'settings:write',
					description: 'Change server settings, integrations, alerts and backups'
				},
				{ id: 'users:admin', description: 'Invite, change and remove users and their sessions' },
				{ id: 'tokens:admin', description: "See and revoke every user's API tokens" },
				{ id: 'terminal:host', description: 'Open terminal sessions (host shell and containers)' }
			]
		}
	],
	presets: [
		{
			id: 'readonly',
			label: 'Read-only',
			description: "Look but don't touch: apps, logs, metrics, activity. No secret values.",
			scopes: VIEWER_SCOPES
		},
		{
			id: 'deployer',
			label: 'Deployer (CI)',
			description: 'Deploy and read one app from CI. Pair it with an app filter.',
			scopes: ['apps:read', 'apps:deploy', 'logs:read']
		},
		{
			id: 'agent-operator',
			label: 'Agent operator',
			description: 'For AI agents: read, deploy, restart, roll back, previews and env var names.',
			scopes: [
				'apps:read',
				'apps:deploy',
				'apps:control',
				'apps:rollback',
				'previews:write',
				'env:read-names',
				'logs:read',
				'metrics:read',
				'events:read'
			]
		}
	],
	roles: [
		{ id: 'admin', description: 'Full control', scopes: ALL_SCOPES },
		{ id: 'deployer', description: 'Ships code', scopes: DEPLOYER_SCOPES },
		{ id: 'viewer', description: 'Read-only', scopes: VIEWER_SCOPES }
	],
	grantable: ALL_SCOPES
};

export const MOCK_TOKENS: ApiToken[] = [
	{
		id: 'tok_ci',
		name: 'github-actions-web',
		prefix: 'tond_pat_3f9a2c1d',
		kind: 'ci',
		scopes: ['apps:read', 'apps:deploy', 'logs:read'],
		app_ids: ['app_running'],
		project_ids: [],
		allowed_ips: [],
		owner_id: 'usr_dev',
		owner_label: 'admin@localhost',
		expires_at: new Date(Date.now() + 60 * 86400_000).toISOString(),
		last_used_at: new Date(Date.now() - 42 * 60_000).toISOString(),
		last_used_ip: '140.82.112.4',
		revoked_at: null,
		created_at: '2025-02-10T09:00:00.000Z'
	},
	{
		id: 'tok_agent',
		name: 'claude-ops-agent',
		prefix: 'tond_pat_b71e0d44',
		kind: 'agent',
		scopes: [
			'apps:read',
			'apps:deploy',
			'apps:control',
			'apps:rollback',
			'previews:write',
			'env:read-names',
			'logs:read',
			'metrics:read',
			'events:read'
		],
		app_ids: [],
		project_ids: ['proj_1'],
		allowed_ips: ['203.0.113.0/24'],
		owner_id: 'usr_dev',
		owner_label: 'admin@localhost',
		expires_at: new Date(Date.now() + 5 * 3600_000).toISOString(),
		last_used_at: new Date(Date.now() - 3 * 60_000).toISOString(),
		last_used_ip: '203.0.113.20',
		revoked_at: null,
		created_at: new Date(Date.now() - 19 * 3600_000).toISOString()
	},
	{
		id: 'tok_old',
		name: 'laptop-scripts',
		prefix: 'tond_pat_0c55e9a1',
		kind: 'personal',
		scopes: ['apps:read', 'logs:read'],
		app_ids: [],
		project_ids: [],
		allowed_ips: [],
		owner_id: 'usr_dev',
		owner_label: 'admin@localhost',
		expires_at: null,
		last_used_at: '2025-01-02T10:00:00.000Z',
		last_used_ip: '192.168.1.42',
		revoked_at: '2025-02-01T12:00:00.000Z',
		created_at: '2024-12-01T09:00:00.000Z'
	}
];

// ── Projects (Phase 6.1) ───────────────────────────────────────────────────

export const MOCK_PROJECTS: Project[] = [
	{
		id: 'proj_1',
		name: 'web-platform',
		description: 'Frontend and backend for the main product',
		env: [
			{ key: 'LOG_LEVEL', masked: '••••••••' },
			{ key: 'NODE_ENV', masked: '••••••••' }
		],
		active_preview_count: 1,
		created_at: '2025-01-08T10:00:00.000Z',
		updated_at: '2025-02-20T09:00:00.000Z'
	},
	{
		id: 'proj_2',
		name: 'data-pipeline',
		description: null,
		env: [],
		active_preview_count: 0,
		created_at: '2025-02-15T14:00:00.000Z',
		updated_at: '2025-02-15T14:00:00.000Z'
	}
];

export const MOCK_TEMPLATES: TemplateCatalogEntry[] = [
	{
		id: 'plausible',
		name: 'Plausible Analytics',
		description: 'Self-hosted privacy-friendly analytics',
		category: 'analytics',
		tags: ['analytics', 'privacy'],
		maintainers: ['tond'],
		project: {
			name: 'plausible',
			description: 'Plausible app and supporting services'
		},
		source: {
			kind: 'image',
			url: 'ghcr.io/plausible/community-edition',
			ref: 'latest'
		},
		deploy: {
			method: 'image',
			default_port: 8000,
			health_check_path: '/',
			volumes: [
				{
					container_path: '/var/lib/plausible',
					description: 'Persistent application data and uploaded assets'
				}
			]
		},
		required_env: [
			{
				key: 'BASE_URL',
				description: 'Public URL for the instance',
				required: true,
				value: null
			},
			{
				key: 'SECRET_KEY_BASE',
				description: 'Secret key used by Plausible',
				required: true,
				value: null
			},
			{
				key: 'DATABASE_URL',
				description: 'Primary PostgreSQL connection string',
				required: true,
				value: '${db.plausible-db.internal_connection_string}'
			}
		],
		optional_env: [],
		dependencies: [
			{
				name: 'plausible-db',
				kind: 'database',
				engine: 'postgres',
				required: true
			}
		],
		recommended_domains: ['stats.example.com'],
		post_deploy_checks: [
			'Open / to verify the dashboard loads',
			'Add your site domain inside Plausible after the first login'
		],
		setup_notes: 'Configure BASE_URL and a PostgreSQL connection before first deploy.'
	}
];

// ── App environment variables (served masked by GET /apps/:id/env) ─────────

export const MOCK_APP_ENV_VARS: Record<string, Record<string, string>> = {
	app_running: {
		DATABASE_URL: 'postgres://api:s3cret@db.internal:5432/api',
		SECRET_KEY: 'sk_live_4f9a2c7e1b',
		LOG_LEVEL: 'info',
		FEATURE_FLAGS: ''
	},
	app_stopped: {
		NEXT_PUBLIC_API_URL: 'https://api.example.com'
	}
};

export const MOCK_PREVIEW_ENV_VARS: Record<string, Record<string, string>> = {
	app_running: { DATABASE_URL: 'postgres://preview:pass@localhost/preview_db' }
};

// ── Apps ───────────────────────────────────────────────────────────────────

export const MOCK_APPS: App[] = [
	{
		id: 'app_running',
		name: 'api-service',
		domain: 'api.example.com',
		source_type: 'git',
		source_url: 'https://github.com/example/api-service',
		status: 'running',
		container_port: 8080,
		host_port: 32001,
		memory_limit: 536870912,
		cpu_limit: 1000000000,
		health_check_path: '/healthz',
		health_check_interval: 30,
		health_failure_threshold: 3,
		health_consecutive_failures: 0,
		volumes: [
			{
				name: 'uploads',
				host_path: '/opt/tond/data/volumes/api-service/uploads',
				container_path: '/app/uploads'
			}
		],
		drain_timeout: 30,
		build_method: 'tbuild',
		auto_deploy: true,
		watch_branch: 'main',
		deploy_key_pub:
			'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM0mock+deploy+key+for+api+service tond@deploy',
		project_id: 'proj_1',
		registry_credential_id: null,
		github_app_id: 'ghapp_1',
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: true,
		preview_shared_keys: ['NODE_ENV'],
		created_at: '2025-01-10T12:00:00.000Z',
		updated_at: '2025-03-01T09:00:00.000Z'
	},
	{
		id: 'app_stopped',
		name: 'web-frontend',
		domain: 'app.example.com',
		source_type: 'git',
		source_url: 'https://github.com/example/web-frontend',
		status: 'stopped',
		container_port: 3000,
		host_port: null,
		memory_limit: 268435456,
		cpu_limit: 500000000,
		health_check_path: null,
		health_check_interval: 30,
		health_failure_threshold: 3,
		health_consecutive_failures: 0,
		volumes: [],
		drain_timeout: 15,
		build_method: null,
		auto_deploy: false,
		watch_branch: null,
		deploy_key_pub: null,
		project_id: 'proj_1',
		registry_credential_id: null,
		github_app_id: null,
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: false,
		preview_shared_keys: [],
		created_at: '2025-01-15T08:00:00.000Z',
		updated_at: '2025-02-28T14:22:00.000Z'
	},
	{
		id: 'app_failed',
		name: 'worker-queue',
		domain: null,
		source_type: 'git',
		source_url: 'https://github.com/example/worker-queue',
		status: 'failed',
		container_port: 4000,
		host_port: null,
		memory_limit: null,
		cpu_limit: null,
		health_check_path: null,
		health_check_interval: 30,
		health_failure_threshold: 3,
		health_consecutive_failures: 4,
		volumes: [],
		drain_timeout: 30,
		build_method: 'tbuild',
		auto_deploy: false,
		watch_branch: null,
		deploy_key_pub: null,
		project_id: 'proj_2',
		registry_credential_id: null,
		github_app_id: null,
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: false,
		preview_shared_keys: [],
		created_at: '2025-02-20T10:00:00.000Z',
		updated_at: '2025-03-05T11:45:00.000Z'
	},
	{
		id: 'app_unhealthy',
		name: 'postgres-proxy',
		domain: 'db.example.com',
		source_type: 'image',
		source_url: null,
		status: 'unhealthy',
		container_port: 5432,
		host_port: 32004,
		memory_limit: 1073741824,
		cpu_limit: 2000000000,
		health_check_path: '/ping',
		health_check_interval: 15,
		health_failure_threshold: 2,
		health_consecutive_failures: 2,
		volumes: [
			{
				name: 'pgdata',
				host_path: '/opt/tond/data/volumes/api-server/pgdata',
				container_path: '/var/lib/postgresql/data'
			}
		],
		drain_timeout: 60,
		build_method: 'dockerfile',
		auto_deploy: false,
		watch_branch: null,
		deploy_key_pub: null,
		project_id: null,
		registry_credential_id: null,
		github_app_id: null,
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: false,
		preview_shared_keys: [],
		created_at: '2025-02-01T00:00:00.000Z',
		updated_at: '2025-03-07T06:00:00.000Z'
	},
	{
		id: 'app_pending',
		name: 'ml-inference',
		domain: null,
		source_type: 'git',
		source_url: 'https://github.com/example/ml-inference',
		status: 'pending',
		container_port: 7000,
		host_port: null,
		memory_limit: 2147483648,
		cpu_limit: 4000000000,
		health_check_path: null,
		health_check_interval: 30,
		health_failure_threshold: 3,
		health_consecutive_failures: 0,
		volumes: [],
		drain_timeout: 30,
		build_method: 'dockerfile',
		auto_deploy: false,
		watch_branch: null,
		deploy_key_pub: null,
		project_id: null,
		registry_credential_id: null,
		github_app_id: null,
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: false,
		preview_shared_keys: [],
		created_at: '2025-03-08T07:00:00.000Z',
		updated_at: '2025-03-08T07:00:00.000Z'
	}
];

// ── Settings ───────────────────────────────────────────────────────────────

export const MOCK_SETTINGS: ServerSettings = {
	server_name: 'Tond Server',
	dashboard_domain: 'dash.example.com',
	preview_domain: 'example-previews.com',
	apps_domain: 'example-apps.com',
	upload_max_bytes: 1024 * 1024 * 1024,
	session_idle_timeout_hours: 12,
	session_max_age_days: 30
};

/** Database links (database_id → app_id): cache-redis is linked to ml-inference. */
export const MOCK_DATABASE_LINKS: { database_id: string; app_id: string }[] = [
	{ database_id: 'db_redis1', app_id: 'app_pending' }
];

// ── Certificates ───────────────────────────────────────────────────────────

const minutesFromNow = (m: number) => new Date(Date.now() + m * 60_000).toISOString();

export const MOCK_CERTIFICATES: Certificate[] = [
	{
		id: 'cert-001',
		domain: 'api.example.com',
		cert_type: 'acme',
		status: 'active',
		issued_at: '2026-08-01T00:00:00Z',
		expires_at: '2026-10-30T00:00:00Z',
		last_error: null,
		last_attempt_at: '2026-08-01T00:00:00Z',
		next_retry_at: null,
		failure_count: 0
	},
	{
		id: 'cert-002',
		domain: 'app.example.com',
		cert_type: 'acme',
		status: 'pending',
		issued_at: '2026-09-20T00:00:00Z',
		expires_at: '2026-12-19T00:00:00Z',
		last_error:
			"DNS for app.example.com points to 1.2.3.4, expected 5.6.7.8. Update the domain's A/AAAA record to this server.",
		last_attempt_at: minutesFromNow(-4),
		next_retry_at: minutesFromNow(12),
		failure_count: 4
	},
	{
		id: 'cert-003',
		domain: 'dash.example.com',
		cert_type: 'acme',
		status: 'failed',
		issued_at: '2026-09-20T00:00:00Z',
		expires_at: '2026-12-19T00:00:00Z',
		last_error:
			'polling order ready: order failed: 403 urn:ietf:params:acme:error:unauthorized: Invalid response from http://dash.example.com/.well-known/acme-challenge/abc: 404',
		last_attempt_at: minutesFromNow(-1),
		next_retry_at: minutesFromNow(1),
		failure_count: 1
	}
];

export const MOCK_ONBOARDING_STATUS: OnboardingStatus = {
	dashboard_domain: MOCK_SETTINGS.dashboard_domain,
	is_fresh_install: false,
	has_projects: true,
	has_apps: true,
	has_databases: true,
	has_cron_jobs: true,
	has_source_integration: true,
	has_successful_deploy: true,
	has_notification_channel: true,
	last_deployment: null,
	server_public_ip: '203.0.113.10',
	domain_status: {
		configured: true,
		expected_target: '203.0.113.10',
		dns_matches_server: true,
		https_ready: false,
		certificate_status: 'pending'
	}
};

// ── System Metrics ─────────────────────────────────────────────────────────

export function makeMockSystemMetrics(): SystemMetrics {
	return {
		cpu_percent: 34.7 + Math.random() * 5,
		mem_used: 3_700_000_000 + Math.random() * 100_000_000,
		mem_total: 8_000_000_000,
		mem_percent: 46.3 + Math.random() * 2,
		disk_used: 42_000_000_000 + Math.random() * 500_000_000,
		disk_total: 160_000_000_000,
		disk_percent: 26.2,
		disk_mount: '/',
		containers_running: 2,
		apps_total: MOCK_APPS.length,
		apps_running: MOCK_APPS.filter((a) => a.status === 'running' || a.status === 'unhealthy')
			.length,
		collected_at: new Date().toISOString()
	};
}

// ── Resource Metrics ───────────────────────────────────────────────────────

const RANGE_SPAN: Record<MetricsRange, number> = {
	'1h': 3600,
	'24h': 86_400,
	'7d': 7 * 86_400,
	'30d': 30 * 86_400
};
const RANGE_STEP: Record<MetricsRange, number> = { '1h': 30, '24h': 180, '7d': 1500, '30d': 7200 };

/**
 * Deterministic-looking series: a daily wave with noise, a 20-minute outage
 * (gap) two thirds in, one crash-restart and one OOM kill. `empty` returns a
 * target that has never been measured.
 */
export function makeMockMetrics(range: string, empty = false): MetricsResponse {
	const r: MetricsRange = (['1h', '24h', '7d', '30d'] as const).includes(range as MetricsRange)
		? (range as MetricsRange)
		: '1h';
	const step = RANGE_STEP[r];
	const to = Math.floor(Date.now() / 1000);
	const from = Math.floor((to - RANGE_SPAN[r]) / step) * step;
	const series: MetricsResponse['series'] = {
		t: [],
		cpu: [],
		cpu_max: [],
		mem: [],
		mem_max: [],
		rx: [],
		tx: [],
		pids: []
	};
	if (empty) {
		return { range: r, step, from, to, mem_limit: null, series, events: [], current: null };
	}
	const n = Math.floor((to - from) / step);
	const gapStart = Math.floor(n * 0.66);
	const gapLen = Math.max(1, Math.round(1200 / step));
	for (let i = 0; i <= n; i++) {
		if (i >= gapStart && i < gapStart + gapLen) continue;
		const t = from + i * step;
		const wave = Math.sin((t / 86_400) * Math.PI * 2) * 0.5 + Math.sin(i / 7) * 0.3;
		const noise = Math.random();
		const cpu = Math.max(0.5, 18 + wave * 12 + noise * 6);
		const mem = 190_000_000 + wave * 35_000_000 + noise * 8_000_000;
		series.t.push(t);
		series.cpu.push(Math.round(cpu * 10) / 10);
		series.cpu_max.push(Math.round((cpu + 10 + noise * 30) * 10) / 10);
		series.mem.push(Math.round(mem));
		series.mem_max.push(Math.round(mem + 12_000_000));
		series.rx.push(Math.round(40_000 + wave * 25_000 + noise * 15_000));
		series.tx.push(Math.round(12_000 + wave * 6_000 + noise * 5_000));
		series.pids.push(14);
	}
	const gapAt = from + gapStart * step;
	const events: MetricsResponse['events'] = [
		{ t: gapAt + 30, kind: 'oom', detail: '1 process killed' },
		{ t: gapAt + gapLen * step, kind: 'restart', detail: 'exit code 137, out of memory' }
	];
	return {
		range: r,
		step,
		from,
		to,
		mem_limit: 512 * 1024 * 1024,
		series,
		events,
		current: {
			t: to,
			cpu: series.cpu.at(-1) ?? 0,
			mem: series.mem.at(-1) ?? 0,
			mem_peak: 301_000_000,
			rx: series.rx.at(-1) ?? 0,
			tx: series.tx.at(-1) ?? 0,
			pids: 14
		}
	};
}

// ── Request metrics (tond-proxy traffic) ───────────────────────────────────

const REQUEST_SPAN: Record<RequestRange, number> = {
	'1h': 3600,
	'6h': 6 * 3600,
	'24h': 86_400,
	'7d': 7 * 86_400,
	'30d': 30 * 86_400,
	'90d': 90 * 86_400
};
const REQUEST_STEP: Record<RequestRange, number> = {
	'1h': 60,
	'6h': 60,
	'24h': 180,
	'7d': 3600,
	'30d': 7200,
	'90d': 18_000
};

/** Interpolated percentile of a latency around `base` ms. */
function latency(base: number, wave: number, q: number): number {
	return Math.round((base * (1 + wave * 0.3) * q + Math.random() * base * 0.2) * 10) / 10;
}

/**
 * Traffic with a daily wave, a burst of 5xx after a deploy two thirds in,
 * and a restart. `scale` sets the requests per second; `empty` is an app
 * without traffic.
 */
export function makeMockRequestMetrics(
	range: string,
	{ empty = false, scale = 4, withDeploys = true } = {}
): RequestMetricsResponse {
	const r: RequestRange = (Object.keys(REQUEST_SPAN) as RequestRange[]).includes(
		range as RequestRange
	)
		? (range as RequestRange)
		: '24h';
	const step = REQUEST_STEP[r];
	const to = Math.floor(Date.now() / 1000);
	const from = Math.floor((to - REQUEST_SPAN[r]) / step) * step;
	const n = Math.floor((to - from) / step);
	const series: RequestMetricsResponse['series'] = {
		t: [],
		rps: [],
		rps_1xx: [],
		rps_2xx: [],
		rps_3xx: [],
		rps_4xx: [],
		rps_5xx: [],
		err_pct: [],
		p50: [],
		p95: [],
		p99: [],
		bytes_in: [],
		bytes_out: []
	};
	const incidentAt = Math.floor(n * 0.66);
	let requests = 0;
	let s2 = 0;
	let s3 = 0;
	let s4 = 0;
	let s5 = 0;
	let bytesOut = 0;
	let bytesIn = 0;
	for (let i = 0; i <= n; i++) {
		const t = from + i * step;
		series.t.push(t);
		if (empty) {
			for (const k of ['rps', 'rps_1xx', 'rps_2xx', 'rps_3xx', 'rps_4xx', 'rps_5xx'] as const)
				series[k].push(0);
			series.err_pct.push(null);
			series.p50.push(null);
			series.p95.push(null);
			series.p99.push(null);
			series.bytes_in.push(0);
			series.bytes_out.push(0);
			continue;
		}
		const wave = Math.sin((t / 86_400) * Math.PI * 2) * 0.5 + 0.5;
		const total = Math.max(0.05, scale * (0.4 + wave) + Math.random() * scale * 0.3);
		const incident = i >= incidentAt && i < incidentAt + Math.max(1, Math.round(900 / step));
		const e5 = incident ? total * 0.18 : total * 0.002;
		const e4 = total * 0.04;
		const e3 = total * 0.08;
		const ok = total - e5 - e4 - e3;
		const round = (v: number) => Math.round(v * 100) / 100;
		series.rps.push(round(total));
		series.rps_1xx.push(0);
		series.rps_2xx.push(round(ok));
		series.rps_3xx.push(round(e3));
		series.rps_4xx.push(round(e4));
		series.rps_5xx.push(round(e5));
		series.err_pct.push(round((e5 / total) * 100));
		const base = incident ? 90 : 24;
		series.p50.push(latency(base, wave, 1));
		series.p95.push(latency(base, wave, 3.2));
		series.p99.push(latency(base, wave, 7));
		series.bytes_in.push(Math.round(total * 420));
		series.bytes_out.push(Math.round(total * 18_000));
		requests += total * step;
		s2 += ok * step;
		s3 += e3 * step;
		s4 += e4 * step;
		s5 += e5 * step;
		bytesIn += total * 420 * step;
		bytesOut += total * 18_000 * step;
	}
	const incidentT = from + incidentAt * step;
	const markers: RequestMetricsResponse['markers'] =
		empty || !withDeploys
			? []
			: [
					{
						t: from + Math.floor(n * 0.2) * step,
						kind: 'deploy',
						detail: 'succeeded',
						deployment_id: 'dep-1'
					},
					{ t: incidentT, kind: 'deploy', detail: 'succeeded', deployment_id: 'dep-2' },
					{ t: incidentT + 600, kind: 'restart', detail: 'exit code 1', deployment_id: null }
				];
	const R = Math.round(requests);
	return {
		range: r,
		step,
		from,
		to,
		series,
		summary: {
			requests: R,
			rps: Math.round((requests / (to - from)) * 100) / 100,
			s1xx: 0,
			s2xx: Math.round(s2),
			s3xx: Math.round(s3),
			s4xx: Math.round(s4),
			s5xx: Math.round(s5),
			s502: Math.round(s5 * 0.7),
			s504: Math.round(s5 * 0.1),
			err_pct: R ? Math.round((s5 / requests) * 10_000) / 100 : null,
			p50: empty ? null : 26.4,
			p95: empty ? null : 88.1,
			p99: empty ? null : 212.5,
			upstream_p50: empty ? null : 23.9,
			upstream_p95: empty ? null : 81.3,
			upstream_p99: empty ? null : 204.2,
			bytes_in: Math.round(bytesIn),
			bytes_out: Math.round(bytesOut)
		},
		current: empty
			? null
			: {
					t: to,
					secs: 60,
					rps: series.rps.at(-1) ?? 0,
					err_pct: series.err_pct.at(-1) ?? null,
					p95: series.p95.at(-1) ?? null
				},
		markers,
		proxy_connected: true
	};
}

/** Server-wide traffic for the home page tile. */
export function makeMockTrafficOverview(range = '24h'): TrafficOverview {
	const running = MOCK_APPS.filter((a) => a.status === 'running' || a.status === 'unhealthy');
	const busiest = running.map((a, i) => {
		const requests = Math.round(180_000 / (i + 1));
		const s5xx = i === 1 ? 1_420 : i === 0 ? 12 : 0;
		return {
			id: a.id,
			name: a.name,
			requests,
			rps: Math.round((requests / 86_400) * 100) / 100,
			s4xx: Math.round(requests * 0.03),
			s5xx,
			err_pct: Math.round((s5xx / requests) * 10_000) / 100,
			p95: 40 + i * 35,
			bytes_out: requests * 18_000
		};
	});
	const total = busiest.reduce((sum, a) => sum + a.requests, 0) + 4_210;
	const s5xx = busiest.reduce((sum, a) => sum + a.s5xx, 0);
	return {
		range: range as RequestRange,
		from: Math.floor(Date.now() / 1000) - 86_400,
		to: Math.floor(Date.now() / 1000),
		total: {
			requests: total,
			rps: Math.round((total / 86_400) * 100) / 100,
			s1xx: 0,
			s2xx: total - s5xx - 9_000,
			s3xx: 3_000,
			s4xx: 6_000,
			s5xx,
			s502: s5xx,
			s504: 0,
			err_pct: Math.round((s5xx / total) * 10_000) / 100,
			p50: 21.3,
			p95: 74.9,
			p99: 190.2,
			upstream_p50: 19.8,
			upstream_p95: 70.1,
			upstream_p99: 182.4,
			bytes_in: total * 400,
			bytes_out: total * 18_000
		},
		unknown_requests: 4_210,
		busiest,
		errors: busiest.filter((a) => a.s5xx > 0).sort((a, b) => b.s5xx - a.s5xx),
		proxy_connected: true
	};
}

/** Access log lines in tond-proxy's format, oldest first. */
export function makeMockAccessLog(count = 300): string[] {
	const paths = ['/', '/api/cart', '/api/products', '/login', '/assets/app.js', '/favicon.ico'];
	const statuses = [200, 200, 200, 200, 304, 301, 404, 200, 500, 200];
	const now = Date.now();
	return Array.from({ length: count }, (_, i) => {
		const t = new Date(now - (count - i) * 7_000).toISOString();
		const path = paths[i % paths.length];
		const status = statuses[i % statuses.length];
		const ms = (4 + ((i * 37) % 180) + (status >= 500 ? 400 : 0)).toFixed(1);
		const bytes = status === 304 ? 0 : 400 + ((i * 131) % 24_000);
		const ip = `203.0.113.${(i * 17) % 250}`;
		const method = path.startsWith('/api/cart') && i % 3 === 0 ? 'POST' : 'GET';
		return `${t} ${method} ${path} ${status} ${ms}ms ${bytes} ${ip}`;
	});
}

// ── Deployments ────────────────────────────────────────────────────────────

/** ISO timestamp `minutes` ago (fixtures read as recent activity). */
function minutesAgo(minutes: number, plusSeconds = 0): string {
	return new Date(Date.now() - minutes * 60_000 + plusSeconds * 1000).toISOString();
}

/** A finished phase list: every visible step succeeded, `seconds` apart. */
function donePhases(start: string, steps: [DeployPhase, string | null, number][]): PhaseRecord[] {
	let t = new Date(start).getTime();
	return steps.map(([phase, label, secs]) => {
		if (label === null) return { phase, status: 'skipped' as const };
		const started_at = new Date(t).toISOString();
		t += secs * 1000;
		return {
			phase,
			status: 'succeeded' as const,
			label,
			started_at,
			finished_at: new Date(t).toISOString()
		};
	});
}

const GIT_STEPS: [DeployPhase, string | null, number][] = [
	['clone', 'Clone repository', 3],
	['build', 'Build image', 71],
	['start', 'Start container', 2],
	['health', 'Health check', 6],
	['release', 'Switch traffic', 4]
];

const DEP3_START = minutesAgo(42);
const DEP2_START = minutesAgo(60 * 26);
const DEP1_START = minutesAgo(60 * 50);
const FAILED_START = minutesAgo(18);

export const MOCK_DEPLOYMENTS: Record<string, Deployment[]> = {
	app_running: [
		{
			id: 'dep_3',
			app_id: 'app_running',
			image_tag: 'tond-api-service:3f2a1b9c1234',
			status: 'succeeded',
			trigger_source: 'webhook',
			trigger_detail: 'GitHub push to main',
			commit_sha: '3f2a1b9c12d4e5f60718293a4b5c6d7e8f901234',
			commit_message: 'Cache product queries for 60 s',
			commit_author: 'Ada Lovelace',
			has_generated_dockerfile: true,
			started_at: DEP3_START,
			finished_at: minutesAgo(42, 86),
			created_at: DEP3_START,
			duration_secs: 86,
			phases: donePhases(DEP3_START, GIT_STEPS),
			failure: null,
			image_pruned: false,
			is_current: true,
			rollback_available: false,
			rollback_unavailable_reason: 'This image is already running'
		},
		{
			id: 'dep_2',
			app_id: 'app_running',
			image_tag: 'tond-api-service:7f3a9b1d0e2c',
			status: 'succeeded',
			trigger_source: 'manual',
			trigger_detail: 'admin@tond.local',
			commit_sha: '7f3a9b1d0e2c4b5a69788f7e6d5c4b3a29181716',
			commit_message: 'Add /v2/orders endpoint',
			commit_author: 'Grace Hopper',
			has_generated_dockerfile: true,
			started_at: DEP2_START,
			finished_at: minutesAgo(60 * 26, 94),
			created_at: DEP2_START,
			duration_secs: 94,
			phases: donePhases(DEP2_START, GIT_STEPS),
			failure: null,
			image_pruned: false,
			is_current: false,
			rollback_available: true,
			rollback_unavailable_reason: null
		},
		{
			id: 'dep_1b',
			app_id: 'app_running',
			image_tag: 'tond-api-service:c0ffee123456',
			status: 'succeeded',
			trigger_source: 'rollback',
			trigger_detail: 'admin@tond.local',
			rollback_of: 'dep_0',
			has_generated_dockerfile: false,
			started_at: minutesAgo(60 * 49),
			finished_at: minutesAgo(60 * 49, 12),
			created_at: minutesAgo(60 * 49),
			duration_secs: 12,
			image_pruned: true,
			is_current: false,
			rollback_available: false,
			rollback_unavailable_reason: "This deployment's image was pruned to save disk space"
		},
		{
			id: 'dep_1',
			app_id: 'app_running',
			image_tag: null,
			status: 'failed',
			trigger_source: 'manual',
			trigger_detail: 'admin@tond.local',
			commit_sha: '1a2b3c4d5e6f708192a3b4c5d6e7f80910111213',
			commit_message: 'Upgrade to TypeScript 6',
			commit_author: 'Ada Lovelace',
			has_generated_dockerfile: true,
			started_at: DEP1_START,
			finished_at: minutesAgo(60 * 50, 48),
			created_at: DEP1_START,
			duration_secs: 48,
			phases: [
				{
					phase: 'clone',
					status: 'succeeded',
					label: 'Clone repository',
					started_at: DEP1_START,
					finished_at: minutesAgo(60 * 50, 3)
				},
				{
					phase: 'build',
					status: 'failed',
					label: 'Build image',
					started_at: minutesAgo(60 * 50, 3),
					finished_at: minutesAgo(60 * 50, 48)
				},
				{ phase: 'start', status: 'pending', label: 'Start container' },
				{ phase: 'health', status: 'pending', label: 'Health check' },
				{ phase: 'release', status: 'pending', label: 'Switch traffic' }
			],
			failure: {
				phase: 'build',
				code: 'build_failed',
				title: 'The build failed (exit code 1)',
				hint: 'The lines below are where the build stopped. Fix the error in your code or Dockerfile and deploy again.',
				detail: 'Docker build failed (exit code: exit status: 1)',
				excerpt: [
					'#12 [builder 5/6] RUN npm run build',
					"#12 4.211 src/orders/total.ts(14,7): error TS2322: Type 'string' is not assignable to type 'number'.",
					'#12 4.388 npm error Lifecycle script `build` failed with error: code 2',
					'#12 ERROR: process "/bin/sh -c npm run build" did not complete successfully: exit code: 2'
				]
			},
			image_pruned: false,
			is_current: false,
			rollback_available: false,
			rollback_unavailable_reason: 'Only successful deployments can be restored'
		}
	],
	app_failed: [
		{
			id: 'dep_wq_1',
			app_id: 'app_failed',
			image_tag: 'tond-worker-queue:9d8c7b6a5f4e',
			status: 'failed',
			trigger_source: 'manual',
			trigger_detail: 'admin@tond.local',
			commit_sha: '9d8c7b6a5f4e3d2c1b0a99887766554433221100',
			commit_message: 'Serve the dashboard with nginx',
			commit_author: 'Grace Hopper',
			has_generated_dockerfile: false,
			started_at: FAILED_START,
			finished_at: minutesAgo(18, 131),
			created_at: FAILED_START,
			duration_secs: 131,
			phases: [
				...donePhases(FAILED_START, GIT_STEPS.slice(0, 3)),
				{
					phase: 'health',
					status: 'failed',
					label: 'Wait for the app to listen',
					started_at: minutesAgo(18, 71),
					finished_at: minutesAgo(18, 131)
				},
				{ phase: 'release', status: 'pending', label: 'Switch traffic' }
			],
			failure: {
				phase: 'health',
				code: 'port_not_listening',
				title: 'Nothing answered on port 4000 within 60s',
				hint: "Tond expected the app to listen on port 4000 inside the container. If it uses another port (nginx listens on 80, many Node apps on 3000), change the app's port; make sure the app binds to 0.0.0.0, not localhost.",
				detail:
					'the new container did not become ready within 60s; the previous version keeps serving',
				excerpt: [
					'/docker-entrypoint.sh: Configuration complete; ready for start up',
					'2025/03/05 11:44:02 [notice] 1#1: nginx/1.27.3',
					'2025/03/05 11:44:02 [notice] 1#1: start worker processes'
				]
			},
			image_pruned: false,
			is_current: false,
			rollback_available: false,
			rollback_unavailable_reason: 'Only successful deployments can be restored'
		}
	]
};

/** Deploy log lines per fixture deployment (live mock deploys add their own). */
export const MOCK_DEPLOYMENT_LOGS: Record<string, DeploymentLogLine[]> = {
	dep_3: mockBuildLog(DEP3_START, 'Cache product queries for 60 s', true),
	dep_2: mockBuildLog(DEP2_START, 'Add /v2/orders endpoint', true),
	dep_1: mockBuildLog(DEP1_START, 'Upgrade to TypeScript 6', false),
	dep_wq_1: [
		{ level: 'info', message: 'Starting deployment...', ts: FAILED_START },
		{
			level: 'info',
			message: 'Commit 9d8c7b6 by Grace Hopper: Serve the dashboard with nginx',
			ts: minutesAgo(18, 2)
		},
		{ level: 'info', message: 'Building with Dockerfile', ts: minutesAgo(18, 3) },
		{
			level: 'info',
			message: '#8 naming to docker.io/library/tond-worker-queue:9d8c7b6a5f4e done',
			ts: minutesAgo(18, 66)
		},
		{ level: 'info', message: 'Starting new container on port 8104:4000', ts: minutesAgo(18, 67) },
		{
			level: 'info',
			message: 'Waiting up to 60s for the app to answer on port 4000',
			ts: minutesAgo(18, 71)
		},
		{
			level: 'error',
			message: 'Deployment failed: Nothing answered on port 4000 within 60s',
			ts: minutesAgo(18, 131)
		}
	]
};

/** A plausible TBuild/docker build log with ANSI colours. */
export function mockBuildLog(start: string, commit: string, ok: boolean): DeploymentLogLine[] {
	const t0 = new Date(start).getTime();
	const at = (s: number) => new Date(t0 + s * 1000).toISOString();
	const lines: [number, string, string][] = [
		[0, 'info', 'Starting deployment...'],
		[0, 'info', 'Using GitHub App installation token for clone'],
		[1, 'info', 'Cloning https://github.com/example/api-service.git (main)'],
		[3, 'info', `Commit 3f2a1b9 by Ada Lovelace: ${commit}`],
		[3, 'info', 'Building with TBuild (tond-scanner)'],
		[3, 'info', 'TBuild detection: stack=node profile=node-npm confidence=high'],
		[4, 'info', '#1 [internal] load build definition from Dockerfile'],
		[4, 'info', '#1 transferring dockerfile: 612B \u001b[32mdone\u001b[0m'],
		[5, 'info', '#5 [builder 1/6] FROM docker.io/library/node:20-alpine'],
		[9, 'info', '#9 [builder 3/6] RUN npm ci'],
		[31, 'info', '#9 21.87 added 412 packages, and audited 413 packages in 21s'],
		[31, 'info', '#9 21.88 \u001b[33m2 moderate severity vulnerabilities\u001b[0m'],
		[32, 'info', '#12 [builder 5/6] RUN npm run build'],
		[33, 'info', '#12 0.412 > api-service@2.3.0 build'],
		[33, 'info', '#12 0.413 > tsc -p tsconfig.build.json']
	];
	if (!ok) {
		lines.push(
			[
				47,
				'info',
				"#12 4.211 src/orders/total.ts(14,7): \u001b[31merror\u001b[0m \u001b[90mTS2322\u001b[0m: Type 'string' is not assignable to type 'number'."
			],
			[48, 'info', '#12 4.388 npm error Lifecycle script `build` failed with error: code 2'],
			[
				48,
				'info',
				'#12 \u001b[31mERROR\u001b[0m: process "/bin/sh -c npm run build" did not complete successfully: exit code: 2'
			],
			[48, 'error', 'Deployment failed: The build failed (exit code 1)']
		);
	} else {
		lines.push(
			[70, 'info', '#12 36.02 \u001b[32m✓\u001b[0m compiled 214 files'],
			[74, 'info', '#15 exporting to image'],
			[75, 'info', '#15 naming to docker.io/library/tond-api-service:3f2a1b9c1234 done'],
			[77, 'info', 'Starting new container on port 8107:8080'],
			[77, 'info', 'Waiting up to 30s for the health check on port 8080/healthz to pass'],
			[82, 'info', 'New container is ready — cutting over traffic'],
			[82, 'info', 'Waiting 3s for proxy to pick up new port before stopping old container'],
			[85, 'info', 'Draining old container 4be1c2a9d0f3 (30s grace)'],
			[86, 'info', 'Deployed successfully! Container: 9a8b7c6d5e4f']
		);
	}
	return lines.map(([s, level, message]) => ({ level, message, ts: at(s) }));
}
export const MOCK_GENERATED_DOCKERFILES: Record<string, string> = {
	dep_3: `FROM node:20-alpine AS builder
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM node:20-alpine
WORKDIR /app
COPY --from=builder /app .
EXPOSE 8080
CMD ["npm", "start"]`,
	dep_2: `FROM node:20-alpine AS builder
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM nginx:alpine
COPY --from=builder /app/dist /usr/share/nginx/html`
};

// ── Deploy Keys ────────────────────────────────────────────────────────────

export const MOCK_DEPLOY_KEYS: Record<string, DeployKey> = {
	app_running: {
		app_id: 'app_running',
		public_key: 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM0mock+deploy+key+for+api+service tond@deploy'
	}
};

// ── Preview Deployments ────────────────────────────────────────────────────

export const MOCK_PREVIEWS: Record<string, PreviewDeployment[]> = {
	app_running: [
		{
			id: 'prev_1',
			app_id: 'app_running',
			project_preview_id: 'pprev_1',
			slug: 'feature-auth',
			subdomain: 'feature-auth-api-service.example-previews.com',
			container_id: 'c0ffee1234567890',
			host_port: 32100,
			image_tag: 'sha-preview1',
			ttl_seconds: 86400,
			status: 'running',
			expires_at: new Date(Date.now() + 18 * 3_600_000).toISOString(),
			created_at: new Date(Date.now() - 6 * 3_600_000).toISOString(),
			url: 'https://feature-auth-api-service.example-previews.com',
			pr_number: 42,
			head_sha: 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2',
			head_ref: 'feature/auth',
			pr_url: 'https://github.com/example/api-service/pull/42',
			comment_id: null
		},
		{
			id: 'prev_2',
			app_id: 'app_running',
			project_preview_id: null,
			slug: 'fix-memory-leak',
			subdomain: 'fix-memory-leak-api-service.example-previews.com',
			container_id: 'deadbeef12345678',
			host_port: 32101,
			image_tag: 'sha-preview2',
			ttl_seconds: 3600,
			status: 'expired',
			expires_at: new Date(Date.now() - 2 * 3_600_000).toISOString(),
			created_at: new Date(Date.now() - 5 * 3_600_000).toISOString(),
			url: null,
			pr_number: null,
			head_sha: null,
			head_ref: null,
			pr_url: null,
			comment_id: null
		}
	]
};

// ── Project Previews ───────────────────────────────────────────────────────

export const MOCK_PROJECT_PREVIEWS: Record<string, ProjectPreview[]> = {
	proj_1: [
		{
			id: 'pprev_1',
			project_id: 'proj_1',
			slug: 'feature-auth',
			ttl_seconds: 86400,
			status: 'running',
			expires_at: new Date(Date.now() + 18 * 3_600_000).toISOString(),
			created_at: new Date(Date.now() - 6 * 3_600_000).toISOString(),
			previews: [
				{
					id: 'prev_1',
					app_id: 'app_running',
					project_preview_id: 'pprev_1',
					slug: 'feature-auth',
					subdomain: 'feature-auth-api-service.example-previews.com',
					container_id: 'c0ffee1234567890',
					host_port: 32100,
					image_tag: 'sha-preview1',
					ttl_seconds: 86400,
					status: 'running',
					expires_at: new Date(Date.now() + 18 * 3_600_000).toISOString(),
					created_at: new Date(Date.now() - 6 * 3_600_000).toISOString(),
					url: 'https://feature-auth-api-service.example-previews.com',
					pr_number: 42,
					head_sha: 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2',
					head_ref: 'feature/auth',
					pr_url: 'https://github.com/example/api-service/pull/42',
					comment_id: null
				},
				{
					id: 'prev_3',
					app_id: 'app_stopped',
					project_preview_id: 'pprev_1',
					slug: 'feature-auth',
					subdomain: 'feature-auth-web-frontend.example-previews.com',
					container_id: 'abcdef0987654321',
					host_port: 32102,
					image_tag: 'sha-preview3',
					ttl_seconds: 86400,
					status: 'running',
					expires_at: new Date(Date.now() + 18 * 3_600_000).toISOString(),
					created_at: new Date(Date.now() - 6 * 3_600_000).toISOString(),
					url: 'https://feature-auth-web-frontend.example-previews.com',
					pr_number: 42,
					head_sha: 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2',
					head_ref: 'feature/auth',
					pr_url: 'https://github.com/example/web-frontend/pull/42',
					comment_id: null
				}
			]
		}
	],
	proj_2: []
};

// ── Alert Rules ────────────────────────────────────────────────────────────

export const MOCK_ALERT_RULES: AlertRule[] = [
	{
		id: 'alert_1',
		app_id: 'app_running',
		name: 'High CPU on api-service',
		metric: 'cpu',
		threshold: 80,
		has_webhook_url: true,
		webhook_host: 'hooks.example.com',
		enabled: true,
		events: [],
		last_fired_at: '2025-03-06T04:12:00.000Z',
		created_at: '2025-01-20T00:00:00.000Z',
		updated_at: '2025-03-06T04:12:00.000Z'
	},
	{
		id: 'alert_2',
		app_id: null,
		name: 'Global Memory Usage',
		metric: 'memory',
		threshold: 85,
		has_webhook_url: true,
		webhook_host: 'hooks.example.com',
		enabled: false,
		events: [],
		last_fired_at: null,
		created_at: '2025-02-01T00:00:00.000Z',
		updated_at: '2025-02-01T00:00:00.000Z'
	},
	{
		id: 'alert_3',
		app_id: null,
		name: 'Deployment Notifications',
		metric: null,
		threshold: 0,
		has_webhook_url: false,
		webhook_host: null,
		enabled: true,
		events: ['deployment_success', 'deployment_failure'],
		last_fired_at: null,
		created_at: '2025-03-01T00:00:00.000Z',
		updated_at: '2025-03-01T00:00:00.000Z'
	}
];

// ── Audit Log ──────────────────────────────────────────────────────────────

type AuditSeed = Omit<AuditLogEntry, 'seq' | 'created_at' | 'outcome' | 'ip'> & {
	minutesAgo: number;
	outcome?: string;
	ip?: string | null;
};

const user = (email: string, id: string) => ({
	actor_kind: 'user',
	actor_id: id,
	actor_label: email
});
const ADMIN = user('admin@localhost', 'usr_dev');
const DANA = user('dana@example.com', 'usr_2');
const app = (id: string, name: string) => ({
	resource_type: 'app',
	resource_id: id,
	resource_name: name
});
const none = { resource_type: null, resource_id: null, resource_name: null };

const AUDIT_SEEDS: AuditSeed[] = [
	{
		...ADMIN,
		...app('app_running', 'api-service'),
		action: 'deploy',
		detail: null,
		minutesAgo: 2,
		ip: '198.51.100.7'
	},
	{
		actor_kind: 'token',
		actor_id: 'tok_agent',
		actor_label: 'claude-ops-agent',
		...app('app_running', 'api-service'),
		action: 'restart',
		detail: null,
		minutesAgo: 9,
		ip: '203.0.113.20'
	},
	{
		actor_kind: 'anonymous',
		actor_id: null,
		actor_label: 'root@example.com',
		...none,
		action: 'login_failed',
		detail: 'wrong email or password',
		minutesAgo: 14,
		outcome: 'failure',
		ip: '185.220.101.3'
	},
	{
		actor_kind: 'token',
		actor_id: 'tok_ci',
		actor_label: 'github-actions-web',
		...app('app_running', 'api-service'),
		action: 'deploy',
		detail: null,
		minutesAgo: 42,
		ip: '140.82.112.4'
	},
	{
		...ADMIN,
		resource_type: 'token',
		resource_id: 'tok_agent',
		resource_name: 'claude-ops-agent',
		action: 'token_create',
		detail: 'kind=agent scopes=apps:read,apps:deploy,apps:control projects=proj_1 expires=in 24h',
		minutesAgo: 19 * 60,
		ip: '198.51.100.7'
	},
	{
		...DANA,
		...app('app_running', 'api-service'),
		action: 'env_update',
		detail: 'set=LOG_LEVEL',
		minutesAgo: 3 * 60,
		ip: '192.0.2.44'
	},
	{
		...ADMIN,
		resource_type: 'user',
		resource_id: 'usr_3',
		resource_name: 'victor@example.com',
		action: 'user_invite',
		detail: 'role=viewer',
		minutesAgo: 26 * 60,
		ip: '198.51.100.7'
	},
	{
		...ADMIN,
		resource_type: 'user',
		resource_id: 'usr_4',
		resource_name: 'former@example.com',
		action: 'user_disable',
		detail: null,
		minutesAgo: 30 * 60,
		ip: '198.51.100.7'
	},
	{
		...DANA,
		resource_type: 'user',
		resource_id: 'usr_2',
		resource_name: 'dana@example.com',
		action: 'login',
		detail: 'with 2FA',
		minutesAgo: 3 * 60 + 5,
		ip: '192.0.2.44'
	},
	{
		actor_kind: 'local_socket',
		actor_id: null,
		actor_label: 'local-socket',
		resource_type: 'settings',
		resource_id: null,
		resource_name: null,
		action: 'host_terminal_enabled',
		detail: null,
		minutesAgo: 50 * 60
	},
	{
		...ADMIN,
		...app('app_stopped', 'web-frontend'),
		action: 'stop',
		detail: null,
		minutesAgo: 52 * 60,
		ip: '198.51.100.7'
	},
	{
		...ADMIN,
		resource_type: 'settings',
		resource_id: null,
		resource_name: null,
		action: 'settings_update',
		detail: 'session_idle_timeout_hours=12',
		minutesAgo: 70 * 60,
		ip: '198.51.100.7'
	},
	{
		...ADMIN,
		...app('app_running', 'api-service'),
		action: 'rollback',
		detail: 'deployment=dep_2 image_tag=tond-api-service:7f3a9b1c2d4e',
		minutesAgo: 90 * 60,
		ip: '198.51.100.7'
	},
	{
		...ADMIN,
		...app('app_stopped', 'web-frontend'),
		action: 'create',
		detail: null,
		minutesAgo: 200 * 60,
		ip: '198.51.100.7'
	}
];

export const MOCK_AUDIT_LOG: AuditLogEntry[] = [...AUDIT_SEEDS]
	.sort((a, b) => a.minutesAgo - b.minutesAgo)
	.map((seed, i) => {
		const { minutesAgo, outcome, ip, ...rest } = seed;
		return {
			...rest,
			seq: AUDIT_SEEDS.length - i + 40,
			created_at: new Date(Date.now() - minutesAgo * 60_000).toISOString(),
			outcome: outcome ?? 'success',
			ip: ip ?? null
		};
	});

// ── Managed Databases (Phase 6.3) ──────────────────────────────────────────

export const MOCK_DATABASES: ManagedDatabase[] = [
	{
		id: 'db_pg1',
		name: 'app-postgres',
		db_type: 'postgres',
		project_id: 'proj_1',
		host_port: 5401,
		status: 'running',
		db_name: 'app_db',
		db_user: 'app_user',
		has_password: true,
		has_root_password: false,
		memory_limit: 268435456,
		custom_external_url: null,
		custom_internal_url: null,
		image_used: 'postgres:16-alpine',
		custom_image: null,
		created_at: '2025-02-01T10:00:00.000Z',
		updated_at: '2025-03-01T09:00:00.000Z',
		public_access: false
	},
	{
		id: 'db_redis1',
		name: 'cache-redis',
		db_type: 'redis',
		project_id: null,
		host_port: 5402,
		status: 'running',
		db_name: null,
		db_user: 'default',
		has_password: true,
		has_root_password: false,
		memory_limit: null,
		custom_external_url: null,
		custom_internal_url: null,
		image_used: 'redis:7-alpine',
		custom_image: null,
		created_at: '2025-02-10T14:00:00.000Z',
		updated_at: '2025-02-10T14:00:00.000Z',
		public_access: false
	},
	{
		id: 'db_mysql1',
		name: 'legacy-mysql',
		db_type: 'mysql',
		project_id: null,
		host_port: null,
		status: 'stopped',
		db_name: 'legacy_db',
		db_user: 'legacy_user',
		has_password: true,
		has_root_password: true,
		memory_limit: 536870912,
		custom_external_url: null,
		custom_internal_url: null,
		image_used: 'mysql:8',
		custom_image: null,
		created_at: '2025-01-20T08:00:00.000Z',
		updated_at: '2025-02-28T12:00:00.000Z',
		public_access: false
	},
	{
		id: 'db_mariadb1',
		name: 'analytics-mariadb',
		db_type: 'mariadb',
		project_id: 'proj_2',
		host_port: 5403,
		status: 'running',
		db_name: 'analytics_db',
		db_user: 'analytics_user',
		has_password: true,
		has_root_password: true,
		memory_limit: 268435456,
		custom_external_url: null,
		custom_internal_url: null,
		image_used: 'mariadb:11',
		custom_image: null,
		created_at: '2025-03-01T12:00:00.000Z',
		updated_at: '2025-03-01T12:00:00.000Z',
		public_access: true
	}
];

/** Stored database passwords (the API never returns them; see the reveal handler). */
export const MOCK_DATABASE_SECRETS: Record<
	string,
	{ db_password: string | null; root_password: string | null }
> = {
	db_pg1: { db_password: 'mock_password_1', root_password: null },
	db_redis1: { db_password: 'mock_redis_password', root_password: null },
	db_mysql1: { db_password: 'mock_password_2', root_password: 'mock_root_password_mysql' },
	db_mariadb1: { db_password: 'mock_mariadb_password', root_password: 'mock_root_password_mariadb' }
};

// ── Cron Jobs (Phase 6.4) ──────────────────────────────────────────────────

export const MOCK_CRON_JOBS: CronJob[] = [
	{
		id: 'cron_1',
		name: 'hourly-report',
		schedule: '0 * * * *',
		image: 'curlimages/curl:latest',
		command: '["curl", "https://example.com/report"]',
		project_id: 'proj_1',
		enabled: true,
		last_run_at: new Date(Date.now() - 45 * 60_000).toISOString(),
		next_run_at: new Date(Date.now() + 15 * 60_000).toISOString(),
		status: 'idle',
		created_at: '2025-02-01T10:00:00.000Z',
		updated_at: new Date(Date.now() - 45 * 60_000).toISOString(),
		timeout_seconds: 60,
		env: [
			{ key: 'API_KEY', masked: '••••••••' },
			{ key: 'REPORT_URL', masked: '••••••••' }
		],
		last_run_status: 'success',
		concurrency_policy: 'skip',
		memory_mb: 128,
		cpu_shares: null,
		timezone: 'America/New_York',
		max_retries: 2,
		retry_delay_seconds: 30,
		volumes: null
	},
	{
		id: 'cron_2',
		name: 'nightly-backup',
		schedule: '0 2 * * *',
		image: 'alpine:latest',
		command: '["sh", "-c", "echo backup done"]',
		project_id: 'proj_2',
		enabled: true,
		last_run_at: new Date(Date.now() - 22 * 3_600_000).toISOString(),
		next_run_at: new Date(Date.now() + 2 * 3_600_000).toISOString(),
		status: 'idle',
		created_at: '2025-01-15T08:00:00.000Z',
		updated_at: new Date(Date.now() - 22 * 3_600_000).toISOString(),
		timeout_seconds: 300,
		env: [],
		last_run_status: 'success',
		concurrency_policy: 'skip',
		memory_mb: 256,
		cpu_shares: 512,
		timezone: null,
		max_retries: 0,
		retry_delay_seconds: 60,
		volumes: [
			{
				name: 'backups',
				host_path: '/opt/tond/data/volumes/_cron/cron_2/backups',
				container_path: '/backup'
			}
		]
	},
	{
		id: 'cron_3',
		name: 'weekly-cleanup',
		schedule: '0 0 * * 0',
		image: 'alpine:latest',
		command: null,
		project_id: null,
		enabled: false,
		last_run_at: null,
		next_run_at: null,
		status: 'idle',
		created_at: '2025-03-01T09:00:00.000Z',
		updated_at: '2025-03-01T09:00:00.000Z',
		timeout_seconds: null,
		env: [],
		last_run_status: null,
		concurrency_policy: 'skip',
		memory_mb: null,
		cpu_shares: null,
		timezone: null,
		max_retries: 0,
		retry_delay_seconds: 60,
		volumes: null
	}
];

export const MOCK_CRON_RUNS: Record<string, CronJobRun[]> = {
	cron_1: [
		{
			id: 'run_1_3',
			cron_job_id: 'cron_1',
			status: 'success',
			exit_code: 0,
			output: 'HTTP/2 200\ncontent-type: application/json\n{"ok":true}',
			started_at: new Date(Date.now() - 45 * 60_000).toISOString(),
			finished_at: new Date(Date.now() - 44 * 60_000 - 8_000).toISOString(),
			attempt: 1
		},
		{
			id: 'run_1_2',
			cron_job_id: 'cron_1',
			status: 'success',
			exit_code: 0,
			output: 'HTTP/2 200',
			started_at: new Date(Date.now() - 105 * 60_000).toISOString(),
			finished_at: new Date(Date.now() - 104 * 60_000 - 5_000).toISOString(),
			attempt: 2
		},
		{
			id: 'run_1_1',
			cron_job_id: 'cron_1',
			status: 'failed',
			exit_code: 6,
			output: 'curl: (6) Could not resolve host: example.com',
			started_at: new Date(Date.now() - 165 * 60_000).toISOString(),
			finished_at: new Date(Date.now() - 164 * 60_000 - 3_000).toISOString(),
			attempt: 1
		}
	],
	cron_2: [
		{
			id: 'run_2_1',
			cron_job_id: 'cron_2',
			status: 'success',
			exit_code: 0,
			output: 'backup done',
			started_at: new Date(Date.now() - 22 * 3_600_000).toISOString(),
			finished_at: new Date(Date.now() - 22 * 3_600_000 + 2_000).toISOString(),
			attempt: 1
		}
	],
	cron_3: []
};

// ── Registry Credentials (Phase 4.3) ──────────────────────────────────────

export const MOCK_REGISTRY_CREDENTIALS: RegistryCredential[] = [
	{
		id: 'cred_ghcr',
		name: 'GitHub Container Registry',
		server: 'ghcr.io',
		username: 'myorg',
		created_at: '2025-01-20T10:00:00.000Z',
		updated_at: '2025-01-20T10:00:00.000Z'
	},
	{
		id: 'cred_dockerhub',
		name: 'Docker Hub',
		server: 'registry-1.docker.io',
		username: 'mydockerhubuser',
		created_at: '2025-02-05T14:00:00.000Z',
		updated_at: '2025-02-05T14:00:00.000Z'
	},
	{
		id: 'cred_gitlab',
		name: 'GitLab Registry',
		server: 'registry.gitlab.com',
		username: 'deploy-token-123',
		created_at: '2025-03-01T09:00:00.000Z',
		updated_at: '2025-03-01T09:00:00.000Z'
	}
];

// ── GitHub Apps (Phase 4.4) ────────────────────────────────────────────────

export const MOCK_GITHUB_APPS: GitHubApp[] = [
	{
		id: 'ghapp_1',
		name: 'tond-dev',
		app_id: 123456,
		client_id: 'Iv1.abc123def456',
		slug: 'tond-dev',
		installation_id: 78901234,
		installed: true,
		created_at: '2025-03-01T00:00:00.000Z',
		updated_at: '2025-03-01T00:00:00.000Z'
	}
];

// ── GitHub Repos + Branches (Phase 4.4) ───────────────────────────────────

export const MOCK_GITHUB_REPOS: Record<string, GitHubRepo[]> = {
	ghapp_1: [
		{
			id: 101,
			full_name: 'example/api-service',
			name: 'api-service',
			private: false,
			clone_url: 'https://github.com/example/api-service.git',
			ssh_url: 'git@github.com:example/api-service.git',
			default_branch: 'main'
		},
		{
			id: 102,
			full_name: 'example/web-frontend',
			name: 'web-frontend',
			private: false,
			clone_url: 'https://github.com/example/web-frontend.git',
			ssh_url: 'git@github.com:example/web-frontend.git',
			default_branch: 'main'
		},
		{
			id: 103,
			full_name: 'example/worker-queue',
			name: 'worker-queue',
			private: true,
			clone_url: 'https://github.com/example/worker-queue.git',
			ssh_url: 'git@github.com:example/worker-queue.git',
			default_branch: 'main'
		},
		{
			id: 104,
			full_name: 'example/ml-inference',
			name: 'ml-inference',
			private: true,
			clone_url: 'https://github.com/example/ml-inference.git',
			ssh_url: 'git@github.com:example/ml-inference.git',
			default_branch: 'develop'
		}
	]
};

export const MOCK_GITHUB_BRANCHES: Record<string, GitHubBranch[]> = {
	'example/api-service': [
		{ name: 'main' },
		{ name: 'develop' },
		{ name: 'feature/auth' },
		{ name: 'fix/memory-leak' }
	],
	'example/web-frontend': [{ name: 'main' }, { name: 'staging' }, { name: 'feature/dark-mode' }],
	'example/worker-queue': [{ name: 'main' }, { name: 'queue-v2' }],
	'example/ml-inference': [{ name: 'develop' }, { name: 'main' }, { name: 'experiment/llm' }]
};

// ── Notification Integrations ──────────────────────────────────────────────

export const MOCK_INTEGRATIONS: Integration[] = [
	{
		id: 'int_telegram_1',
		name: 'Ops Telegram',
		kind: 'telegram',
		config: { chat_id: '-1001234567890' },
		saved_secrets: ['bot_token'],
		enabled: true,
		created_at: '2025-03-01T10:00:00.000Z',
		updated_at: '2025-03-01T10:00:00.000Z'
	},
	{
		id: 'int_discord_1',
		name: 'Discord #alerts',
		kind: 'discord',
		config: {},
		saved_secrets: ['webhook_url'],
		enabled: true,
		created_at: '2025-03-05T12:00:00.000Z',
		updated_at: '2025-03-05T12:00:00.000Z'
	},
	{
		id: 'int_slack_1',
		name: 'Slack #incidents',
		kind: 'slack',
		config: {},
		saved_secrets: ['webhook_url'],
		enabled: false,
		created_at: '2025-03-10T08:00:00.000Z',
		updated_at: '2025-03-10T08:00:00.000Z'
	},
	{
		id: 'int_email_1',
		name: 'Team Email',
		kind: 'email',
		config: {
			from_name: 'Tond Alerts',
			from_addr: 'alerts@example.com',
			smtp_host: 'smtp.example.com',
			smtp_port: 587,
			smtp_user: 'alerts@example.com',
			smtp_encryption: 'starttls',
			to_addrs: ['ops@example.com', 'admin@example.com']
		},
		saved_secrets: ['smtp_password'],
		enabled: true,
		created_at: '2025-03-12T09:00:00.000Z',
		updated_at: '2025-03-12T09:00:00.000Z'
	}
];

// ── Storage Providers ──────────────────────────────────────────────────────

export const MOCK_STORAGE_PROVIDERS: StorageProvider[] = [
	{
		id: 'sp_r2_1',
		name: 'Cloudflare R2 (prod)',
		kind: 's3',
		config: {
			endpoint: 'https://abc123.r2.cloudflarestorage.com',
			region: 'auto',
			bucket: 'tond-backups',
			prefix: 'prod/',
			access_key_id: 'r2key123abc'
		},
		created_at: '2025-03-01T10:00:00.000Z',
		updated_at: '2025-03-01T10:00:00.000Z'
	},
	{
		id: 'sp_s3_1',
		name: 'AWS S3 (offsite-dr)',
		kind: 's3',
		config: {
			endpoint: null,
			region: 'eu-west-1',
			bucket: 'tond-dr-backups',
			prefix: null,
			access_key_id: 'AKIAIOSFODNN7EXAMPLE'
		},
		created_at: '2025-03-10T14:00:00.000Z',
		updated_at: '2025-03-10T14:00:00.000Z'
	}
];

// ── Snapshots ──────────────────────────────────────────────────────────────

export const MOCK_SNAPSHOTS: Snapshot[] = [
	{
		id: 'snap_1',
		name: 'Scheduled 2026-03-31 03:00',
		status: 'completed',
		trigger: 'scheduled',
		scope: 'full',
		trigger_app: null,
		deployment_id: null,
		parent_snapshot_id: null,
		created_at: new Date(new Date().setHours(3, 0, 0, 0)).toISOString(),
		completed_at: new Date(new Date().setHours(3, 4, 0, 0)).toISOString(),
		delta_bytes: 2_202_009,
		total_bytes: 98_304_000,
		local_path: '/opt/tond/snapshots/snap_1',
		remote_key: 'snapshots/snap_1.tondsnap',
		storage_provider_id: 'sp_r2_1',
		checksum_sha256: 'abc123def456',
		error_message: null,
		notes: null,
		remote_status: 'uploaded',
		remote_error: null,
		archive_bytes: 41_943_040
	},
	{
		id: 'snap_2',
		name: 'Pre-deploy: api-service',
		status: 'completed',
		trigger: 'pre_deploy',
		scope: 'app:api-service',
		trigger_app: 'api-service',
		deployment_id: 'dep_3',
		parent_snapshot_id: 'snap_1',
		created_at: new Date(new Date().setHours(11, 42, 0, 0)).toISOString(),
		completed_at: new Date(new Date().setHours(11, 42, 8, 0)).toISOString(),
		delta_bytes: 4_408,
		total_bytes: 1_048_576,
		local_path: '/opt/tond/snapshots/snap_2',
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: null,
		notes: null,
		remote_status: null,
		remote_error: null,
		archive_bytes: null
	},
	{
		id: 'snap_3',
		name: 'Pre-deploy: web-frontend',
		status: 'completed',
		trigger: 'pre_deploy',
		scope: 'app:web-frontend',
		trigger_app: 'web-frontend',
		deployment_id: 'dep_2',
		parent_snapshot_id: 'snap_1',
		created_at: new Date(new Date().setHours(14, 15, 0, 0)).toISOString(),
		completed_at: new Date(new Date().setHours(14, 15, 5, 0)).toISOString(),
		delta_bytes: 1_153,
		total_bytes: 524_288,
		local_path: '/opt/tond/snapshots/snap_3',
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: null,
		notes: null,
		remote_status: null,
		remote_error: null,
		archive_bytes: null
	},
	{
		id: 'snap_4',
		name: 'Scheduled 2026-03-30 03:00',
		status: 'completed',
		trigger: 'scheduled',
		scope: 'full',
		trigger_app: null,
		deployment_id: null,
		parent_snapshot_id: null,
		created_at: new Date(Date.now() - 24 * 3_600_000).toISOString(),
		completed_at: new Date(Date.now() - 24 * 3_600_000 + 240_000).toISOString(),
		delta_bytes: 1_887_436,
		total_bytes: 96_102_400,
		local_path: '/opt/tond/snapshots/snap_4',
		remote_key: 'snapshots/snap_4.tondsnap',
		storage_provider_id: 'sp_r2_1',
		checksum_sha256: 'def456ghi789',
		error_message: null,
		notes: null,
		remote_status: 'uploaded',
		remote_error: null,
		archive_bytes: 41_943_040
	},
	{
		id: 'snap_5',
		name: 'Pre-deploy: api-service',
		status: 'completed',
		trigger: 'pre_deploy',
		scope: 'app:api-service',
		trigger_app: 'api-service',
		deployment_id: 'dep_2',
		parent_snapshot_id: 'snap_4',
		created_at: new Date(Date.now() - 24 * 3_600_000 + 6 * 3_600_000).toISOString(),
		completed_at: new Date(Date.now() - 24 * 3_600_000 + 6 * 3_600_000 + 6000).toISOString(),
		delta_bytes: 3_983,
		total_bytes: 983_040,
		local_path: '/opt/tond/snapshots/snap_5',
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: null,
		notes: null,
		remote_status: null,
		remote_error: null,
		archive_bytes: null
	},
	{
		id: 'snap_failed',
		name: 'Manual',
		status: 'failed',
		trigger: 'manual',
		scope: 'full',
		trigger_app: null,
		deployment_id: null,
		parent_snapshot_id: null,
		created_at: new Date(Date.now() - 24 * 3_600_000 + 12 * 3_600_000).toISOString(),
		completed_at: new Date(Date.now() - 24 * 3_600_000 + 12 * 3_600_000 + 5000).toISOString(),
		delta_bytes: null,
		total_bytes: null,
		local_path: null,
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: 'Disk write error: no space left on device',
		notes: null,
		remote_status: null,
		remote_error: null,
		archive_bytes: null
	}
];

// ── Backup Config ──────────────────────────────────────────────────────────

export const MOCK_BACKUP_CONFIG: BackupConfig = {
	enabled: true,
	full_schedule_cron: '0 3 * * *',
	full_retention_count: 7,
	pre_deploy_enabled: true,
	pre_deploy_retention_per_app: 5,
	keep_local: true,
	storage_provider_id: 'sp_r2_1'
};

// ── Tond measuring itself ──────────────────────────────────────────────────

const loop = (name: string, extra: Partial<LoopStats> = {}): LoopStats => ({
	name,
	wakeups: 12,
	runs: 12,
	errors: 0,
	last_run_at: new Date(Date.now() - 4 * 60_000).toISOString(),
	last_duration_ms: 3.2,
	last_error: null,
	max_lag_ms: 1.4,
	next_wake_at: new Date(Date.now() + 56 * 60_000).toISOString(),
	...extra
});

export function makeMockSystemSelf(): SystemSelf {
	const now = Date.now();
	return {
		version: '0.1.0',
		server: {
			rss_bytes: 13.4 * 1024 * 1024,
			cpu_seconds: 41.2,
			open_fds: 23,
			threads: 6,
			context_switches: 48210,
			cpu_percent: 0.05,
			uptime_secs: 3 * 86_400 + 4 * 3600,
			tokio_alive_tasks: 14,
			tokio_workers: 2,
			tokio_parks: 9120
		},
		loops: [
			loop('health', {
				wakeups: 412,
				runs: 412,
				last_run_at: new Date(now - 12_000).toISOString(),
				last_duration_ms: 18.5,
				next_wake_at: new Date(now + 18_000).toISOString()
			}),
			loop('metrics', {
				last_run_at: new Date(now - 21_000).toISOString(),
				last_duration_ms: 42.1,
				next_wake_at: new Date(now + 9_000).toISOString()
			}),
			loop('alerts', { next_wake_at: null }),
			loop('cron', { next_wake_at: new Date(now + 26 * 60_000).toISOString() }),
			loop('janitor', {
				last_run_at: new Date(now - 38 * 60_000).toISOString(),
				last_duration_ms: 64.8,
				next_wake_at: new Date(now + 22 * 60_000).toISOString()
			}),
			loop('backup-scheduler', { next_wake_at: new Date(now + 9 * 3_600_000).toISOString() }),
			loop('preview-cleanup'),
			loop('proxy-link', { next_wake_at: null }),
			loop('route-nudge', { last_duration_ms: 11.3, next_wake_at: null })
		],
		db: {
			size_bytes: 1.8 * 1024 * 1024,
			wal_bytes: 96 * 1024,
			queries_total: 18342,
			slow_total: 1,
			slow_threshold_ms: 100,
			recent_slow: [
				{
					sql: 'DELETE FROM deploy_logs WHERE rowid IN (SELECT rowid FROM deploy_logs WHERE app_id = ? AND created_at < ? LIMIT ?)',
					elapsed_ms: 131.2,
					at: new Date(now - 38 * 60_000).toISOString()
				}
			],
			top_statements: [
				{
					sql: 'SELECT * FROM apps WHERE status IN (?, ?) AND health_check_path IS NOT NULL',
					count: 412
				}
			]
		},
		http_bucket_bounds_ms: [1, 5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000],
		http: [
			{
				method: 'GET',
				route: '/api/apps',
				count: 120,
				errors: 0,
				sum_ms: 240,
				buckets: [60, 55, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
				p50_ms: 5,
				p95_ms: 5
			}
		],
		build_queue: { build_slots: 1, active: 0, queued: 0 },
		proxy: {
			connected: true,
			cpu_percent: 0.02,
			stats: {
				pid: 812,
				rss_bytes: 9.6 * 1024 * 1024,
				cpu_seconds: 12.7,
				open_fds: 31,
				threads: 5,
				context_switches: 90111,
				uptime_secs: 3 * 86_400 + 4 * 3600,
				route_version: 57,
				routes: 6,
				reloads: 57,
				loops: [
					loop('route-sync', { wakeups: 0, runs: 58, last_duration_ms: 1.1, next_wake_at: null }),
					loop('route-poll', {
						wakeups: 4560,
						last_run_at: new Date(now - 31_000).toISOString(),
						last_duration_ms: 0.4,
						next_wake_at: new Date(now + 29_000).toISOString()
					}),
					loop('certificates', {
						last_run_at: new Date(now - 2 * 3_600_000).toISOString(),
						last_duration_ms: 2.3,
						next_wake_at: new Date(now + 10 * 3_600_000).toISOString()
					})
				]
			}
		}
	};
}
