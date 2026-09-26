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
	DeployKey,
	GitHubApp,
	GitHubRepo,
	GitHubBranch,
	Integration,
	LoopStats,
	ManagedDatabase,
	SystemSelf,
	TemplateCatalogEntry,
	MetricPoint,
	OnboardingStatus,
	PreviewDeployment,
	Project,
	ProjectPreview,
	RegistryCredential,
	ServerSettings,
	Snapshot,
	StorageProvider,
	SystemMetrics,
	User
} from '$lib/api';

// ── Users ──────────────────────────────────────────────────────────────────

export const MOCK_USER: User = {
	id: 'usr_dev',
	email: 'admin@localhost',
	role: 'admin',
	totp_enabled: false,
	created_at: '2025-01-01T00:00:00.000Z'
};

export const MOCK_USERS: User[] = [
	MOCK_USER,
	{
		id: 'usr_2',
		email: 'member@localhost',
		role: 'member',
		totp_enabled: false,
		created_at: '2025-02-01T00:00:00.000Z'
	}
];

// ── Projects (Phase 6.1) ───────────────────────────────────────────────────

export const MOCK_PROJECTS: Project[] = [
	{
		id: 'proj_1',
		name: 'web-platform',
		description: 'Frontend and backend for the main product',
		env_vars: { NODE_ENV: 'production', LOG_LEVEL: 'info' },
		active_preview_count: 1,
		created_at: '2025-01-08T10:00:00.000Z',
		updated_at: '2025-02-20T09:00:00.000Z'
	},
	{
		id: 'proj_2',
		name: 'data-pipeline',
		description: null,
		env_vars: {},
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
		preview_excluded_keys: ['DATABASE_URL', 'SECRET_KEY'],
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
		preview_excluded_keys: [],
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
		preview_excluded_keys: [],
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
		preview_excluded_keys: [],
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
		preview_excluded_keys: [],
		created_at: '2025-03-08T07:00:00.000Z',
		updated_at: '2025-03-08T07:00:00.000Z'
	}
];

// ── Settings ───────────────────────────────────────────────────────────────

export const MOCK_SETTINGS: ServerSettings = {
	server_name: 'Tond Server',
	dashboard_domain: 'dash.example.com',
	preview_domain: 'example-previews.com'
};

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
		containers_running: 2,
		apps_total: MOCK_APPS.length,
		apps_running: MOCK_APPS.filter((a) => a.status === 'running' || a.status === 'unhealthy')
			.length,
		collected_at: new Date().toISOString()
	};
}

// ── App Metrics ────────────────────────────────────────────────────────────

export function makeMockAppMetrics(period: string): MetricPoint[] {
	const points = period === '7d' ? 168 : period === '24h' ? 48 : 12;
	const now = Date.now();
	const intervalMs = period === '7d' ? 3_600_000 : period === '24h' ? 1_800_000 : 300_000;
	return Array.from({ length: points }, (_, i) => {
		const t = now - (points - 1 - i) * intervalMs;
		const wave = Math.sin((i / points) * Math.PI * 4);
		return {
			cpu_percent: Math.max(0, 20 + wave * 15 + Math.random() * 5),
			mem_used: 180_000_000 + wave * 30_000_000 + Math.random() * 10_000_000,
			mem_limit: 512_000_000,
			mem_percent: Math.max(0, 35 + wave * 6 + Math.random() * 2),
			net_rx: Math.max(0, 1_000_000 + Math.random() * 500_000),
			net_tx: Math.max(0, 200_000 + Math.random() * 100_000),
			restarts: i === Math.floor(points * 0.6) ? 1 : 0,
			collected_at: new Date(t).toISOString()
		};
	});
}

// ── Deployments ────────────────────────────────────────────────────────────

export const MOCK_DEPLOYMENTS: Record<string, Deployment[]> = {
	app_running: [
		{
			id: 'dep_3',
			app_id: 'app_running',
			image_tag: 'tond-api-service:3f2a1b9c1234',
			status: 'succeeded',
			trigger_source: 'webhook',
			has_generated_dockerfile: true,
			started_at: '2025-03-01T08:55:00.000Z',
			finished_at: '2025-03-01T09:02:00.000Z',
			created_at: '2025-03-01T08:55:00.000Z',
			image_pruned: false,
			kind: 'deploy',
			error: null,
			superseded_by: null,
			queue_position: null,
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
			has_generated_dockerfile: true,
			started_at: '2025-02-15T14:10:00.000Z',
			finished_at: '2025-02-15T14:17:00.000Z',
			created_at: '2025-02-15T14:10:00.000Z',
			image_pruned: false,
			kind: 'deploy',
			error: null,
			superseded_by: null,
			queue_position: null,
			is_current: false,
			rollback_available: true,
			rollback_unavailable_reason: null
		},
		{
			id: 'dep_1b',
			app_id: 'app_running',
			image_tag: 'tond-api-service:c0ffee123456',
			status: 'succeeded',
			trigger_source: 'manual',
			has_generated_dockerfile: false,
			started_at: '2025-02-12T09:00:00.000Z',
			finished_at: '2025-02-12T09:05:00.000Z',
			created_at: '2025-02-12T09:00:00.000Z',
			image_pruned: true,
			kind: 'deploy',
			error: null,
			superseded_by: null,
			queue_position: null,
			is_current: false,
			rollback_available: false,
			rollback_unavailable_reason: "This deployment's image was pruned to save disk space"
		},
		{
			id: 'dep_1',
			app_id: 'app_running',
			image_tag: 'tond-api-service:1a2b3c4d5e6f',
			status: 'failed',
			trigger_source: 'manual',
			has_generated_dockerfile: false,
			started_at: '2025-02-10T10:00:00.000Z',
			finished_at: '2025-02-10T10:03:00.000Z',
			created_at: '2025-02-10T10:00:00.000Z',
			image_pruned: true,
			kind: 'deploy',
			error: 'Interrupted: Tond restarted while this was in progress. Deploy again to retry.',
			superseded_by: null,
			queue_position: null,
			is_current: false,
			rollback_available: false,
			rollback_unavailable_reason: 'Only successful deployments can be restored'
		}
	]
};

// Deployment dep_3 superseded a webhook deploy that was still waiting.
MOCK_DEPLOYMENTS.app_running.splice(1, 0, {
	id: 'dep_2b',
	app_id: 'app_running',
	image_tag: null,
	status: 'superseded',
	trigger_source: 'webhook',
	has_generated_dockerfile: false,
	started_at: '2025-03-01T08:54:30.000Z',
	finished_at: '2025-03-01T08:55:00.000Z',
	created_at: '2025-03-01T08:54:30.000Z',
	image_pruned: false,
	kind: 'deploy',
	error: 'Replaced by a newer deploy before it started',
	superseded_by: 'dep_3',
	queue_position: null,
	is_current: false,
	rollback_available: false,
	rollback_unavailable_reason: 'Only successful deployments can be restored'
});

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
		webhook_url: 'https://hooks.example.com/cpu-alert',
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
		webhook_url: 'https://hooks.example.com/memory-alert',
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
		webhook_url: '',
		enabled: true,
		events: ['deployment_success', 'deployment_failure'],
		last_fired_at: null,
		created_at: '2025-03-01T00:00:00.000Z',
		updated_at: '2025-03-01T00:00:00.000Z'
	}
];

// ── Audit Log ──────────────────────────────────────────────────────────────

export const MOCK_AUDIT_LOG: AuditLogEntry[] = [
	{
		id: 'log_10',
		app_id: 'app_running',
		app_name: 'api-service',
		action: 'deploy',
		actor: 'admin@localhost',
		detail: 'Deployed sha-abc123f',
		created_at: '2025-03-01T09:02:00.000Z'
	},
	{
		id: 'log_9',
		app_id: 'app_running',
		app_name: 'api-service',
		action: 'env_update',
		actor: 'admin@localhost',
		detail: 'Updated environment variables',
		created_at: '2025-02-28T16:30:00.000Z'
	},
	{
		id: 'log_8',
		app_id: 'app_stopped',
		app_name: 'web-frontend',
		action: 'stop',
		actor: 'admin@localhost',
		detail: null,
		created_at: '2025-02-28T14:22:00.000Z'
	},
	{
		id: 'log_7',
		app_id: 'app_stopped',
		app_name: 'web-frontend',
		action: 'deploy',
		actor: 'admin@localhost',
		detail: 'Deployed sha-frontend1',
		created_at: '2025-02-28T14:00:00.000Z'
	},
	{
		id: 'log_6',
		app_id: 'app_failed',
		app_name: 'worker-queue',
		action: 'deploy',
		actor: 'member@localhost',
		detail: 'Deploy failed: build error',
		created_at: '2025-03-05T11:45:00.000Z'
	},
	{
		id: 'log_5',
		app_id: null,
		app_name: null,
		action: 'settings_update',
		actor: 'admin@localhost',
		detail: 'Updated ACME email',
		created_at: '2025-02-20T09:00:00.000Z'
	},
	{
		id: 'log_4',
		app_id: 'app_running',
		app_name: 'api-service',
		action: 'rollback',
		actor: 'admin@localhost',
		detail: 'Rolled back to sha-7f3a9b1',
		created_at: '2025-02-18T11:00:00.000Z'
	},
	{
		id: 'log_3',
		app_id: 'app_unhealthy',
		app_name: 'postgres-proxy',
		action: 'health_update',
		actor: 'admin@localhost',
		detail: 'Health check path set to /ping',
		created_at: '2025-02-15T09:30:00.000Z'
	},
	{
		id: 'log_2',
		app_id: 'app_stopped',
		app_name: 'web-frontend',
		action: 'create',
		actor: 'admin@localhost',
		detail: 'App created',
		created_at: '2025-01-15T08:00:00.000Z'
	},
	{
		id: 'log_1',
		app_id: 'app_running',
		app_name: 'api-service',
		action: 'create',
		actor: 'admin@localhost',
		detail: 'App created',
		created_at: '2025-01-10T12:00:00.000Z'
	}
];

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
		db_password: 'mock_password_1',
		root_password: null,
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
		db_password: 'mock_redis_password',
		root_password: null,
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
		db_password: 'mock_password_2',
		root_password: 'mock_root_password_mysql',
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
		db_password: 'mock_mariadb_password',
		root_password: 'mock_root_password_mariadb',
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
		env: { REPORT_URL: 'https://example.com/report', API_KEY: 'secret123' },
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
		env: null,
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
		env: null,
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
		config: { bot_token: '123456789:AABBccDDeeffGGhhIIjjKKll', chat_id: '-1001234567890' },
		enabled: true,
		created_at: '2025-03-01T10:00:00.000Z',
		updated_at: '2025-03-01T10:00:00.000Z'
	},
	{
		id: 'int_discord_1',
		name: 'Discord #alerts',
		kind: 'discord',
		config: { webhook_url: 'https://discord.com/api/webhooks/1234567890/mock-token' },
		enabled: true,
		created_at: '2025-03-05T12:00:00.000Z',
		updated_at: '2025-03-05T12:00:00.000Z'
	},
	{
		id: 'int_slack_1',
		name: 'Slack #incidents',
		kind: 'slack',
		config: { webhook_url: 'https://hooks.slack.com/services/T00/B00/mock-token' },
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
			smtp_password: 'smtp-mock-password',
			smtp_encryption: 'starttls',
			to_addrs: ['ops@example.com', 'admin@example.com']
		},
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
		remote_key: 'prod/snap_1.tar.gz',
		storage_provider_id: 'sp_r2_1',
		checksum_sha256: 'abc123def456',
		error_message: null,
		notes: null
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
		notes: null
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
		notes: null
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
		remote_key: 'prod/snap_4.tar.gz',
		storage_provider_id: 'sp_r2_1',
		checksum_sha256: 'def456ghi789',
		error_message: null,
		notes: null
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
		notes: null
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
		notes: null
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
