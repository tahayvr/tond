// Mock data for a Docker Compose app ("shop-stack"): its services and a few
// deployments with pinned per-service images, so the Services tab, the
// compose file editor, per-service deploy progress and compose rollback can
// be developed without a server.

import type { App, ComposeService, Deployment, DeploymentService, PhaseRecord } from '$lib/api';

const minutesAgo = (m: number, plusSecs = 0) =>
	new Date(Date.now() - m * 60_000 + plusSecs * 1000).toISOString();

export const MOCK_COMPOSE_FILE = `services:
  web:
    image: ghcr.io/acme/shop-web:2.4
    expose: ["3000"]
    environment:
      REDIS_URL: redis://cache:6379
    depends_on: [cache, db]
  admin:
    build: ./admin
    expose: ["8080"]
  cache:
    image: redis:7-alpine
    volumes: ["cache-data:/data"]
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 5s
  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_PASSWORD: \${POSTGRES_PASSWORD:-change-me}
    volumes: ["./pgdata:/var/lib/postgresql/data"]
    healthcheck:
      test: ["CMD", "pg_isready"]
  migrate:
    image: ghcr.io/acme/shop-web:2.4
    command: ["npm", "run", "migrate"]
    restart: "no"
volumes:
  cache-data: {}
`;

export const MOCK_COMPOSE_APP: App = {
	id: 'app_compose',
	name: 'shop-stack',
	domain: 'shop.example.com',
	source_type: 'compose',
	source_url: null,
	branch: null,
	status: 'running',
	container_port: 3000,
	host_port: null,
	memory_limit: null,
	cpu_limit: null,
	health_check_path: null,
	health_check_interval: 30,
	health_failure_threshold: 3,
	health_consecutive_failures: 0,
	volumes: [],
	drain_timeout: 30,
	build_method: null,
	auto_deploy: false,
	watch_branch: null,
	deploy_key_pub: null,
	project_id: null,
	registry_credential_id: null,
	github_app_id: null,
	dockerfile_content: null,
	compose_content: MOCK_COMPOSE_FILE,
	compose_path: null,
	base_dir: null,
	static_build: false,
	preview_enabled: false,
	preview_shared_keys: [],
	readiness: 'http',
	ready_after_secs: 10,
	created_at: '2025-03-02T10:00:00.000Z',
	updated_at: '2025-03-09T10:00:00.000Z'
};

const MiB = 1_048_576;

export const MOCK_COMPOSE_SERVICES: Record<string, ComposeService[]> = {
	app_compose: [
		{
			name: 'web',
			image: 'ghcr.io/acme/shop-web:2.4',
			builds: false,
			ports: [3000],
			has_healthcheck: false,
			published_port: 3000,
			host_port: 8300,
			redeploy_required: false,
			domains: ['shop.example.com', 'www.shop.example.com'],
			status: 'running',
			health: null,
			exit_code: null,
			container_id: '4be1c2a9d0f3',
			running_image: 'tond-shop-stack/web:9c1e0a7b2d4f',
			image_digest: 'ghcr.io/acme/shop-web@sha256:1f4c2a9be07d3c55aa0021c1',
			cpu: 240,
			mem: 142 * MiB
		},
		{
			name: 'admin',
			image: null,
			builds: true,
			ports: [8080],
			has_healthcheck: false,
			published_port: 8080,
			host_port: 8301,
			redeploy_required: false,
			domains: ['admin.shop.example.com'],
			status: 'running',
			health: null,
			exit_code: null,
			container_id: '77aa01be93c1',
			running_image: 'tond-shop-stack/admin:9c1e0a7b2d4f',
			image_digest: 'sha256:8a0d91b7cc3e2f1b09aa7710',
			cpu: 35,
			mem: 61 * MiB
		},
		{
			name: 'cache',
			image: 'redis:7-alpine',
			builds: false,
			ports: [],
			has_healthcheck: true,
			published_port: null,
			host_port: null,
			redeploy_required: false,
			domains: [],
			status: 'running',
			health: 'healthy',
			exit_code: null,
			container_id: 'c0ffee00a1b2',
			running_image: 'tond-shop-stack/cache:9c1e0a7b2d4f',
			image_digest: 'redis@sha256:4f1c2a3b9e01d7c55a0021c1',
			cpu: 12,
			mem: 9 * MiB
		},
		{
			name: 'db',
			image: 'postgres:16-alpine',
			builds: false,
			ports: [],
			has_healthcheck: true,
			published_port: null,
			host_port: null,
			redeploy_required: false,
			domains: [],
			status: 'running',
			health: 'healthy',
			exit_code: null,
			container_id: 'deadbeef0042',
			running_image: 'tond-shop-stack/db:9c1e0a7b2d4f',
			image_digest: 'postgres@sha256:b9e0117c2d4fa5510c3e8812',
			cpu: 88,
			mem: 212 * MiB
		},
		{
			name: 'migrate',
			image: 'ghcr.io/acme/shop-web:2.4',
			builds: false,
			ports: [],
			has_healthcheck: false,
			published_port: null,
			host_port: null,
			redeploy_required: false,
			domains: [],
			status: 'exited',
			health: null,
			exit_code: 0,
			container_id: 'aa11bb22cc33',
			running_image: 'tond-shop-stack/migrate:9c1e0a7b2d4f',
			image_digest: 'ghcr.io/acme/shop-web@sha256:1f4c2a9be07d3c55aa0021c1',
			cpu: null,
			mem: null
		}
	]
};

const COMPOSE_STEPS: [PhaseRecord['phase'], string | null, number][] = [
	['clone', null, 0],
	['build', 'Build and pull service images', 48],
	['start', 'Start services', 9],
	['health', 'Wait for services to be ready', 14],
	['release', 'Switch traffic', 2]
];

function phasesFrom(start: string, steps: typeof COMPOSE_STEPS): PhaseRecord[] {
	let t = Date.parse(start);
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

/** The pinned images of a compose release. */
export function mockReleaseServices(release: string, status = 'running'): DeploymentService[] {
	const svc = (
		service: string,
		source: 'build' | 'pull',
		sourceImage: string,
		digest: string
	): DeploymentService => ({
		service,
		source,
		source_image: sourceImage,
		image: `tond-shop-stack/${service}:${release}`,
		digest,
		status
	});
	return [
		svc(
			'admin',
			'build',
			'/opt/tond/data/compose/shop-stack/admin',
			'sha256:8a0d91b7cc3e2f1b09aa7710'
		),
		svc('cache', 'pull', 'redis:7-alpine', 'redis@sha256:4f1c2a3b9e01d7c55a0021c1'),
		svc('db', 'pull', 'postgres:16-alpine', 'postgres@sha256:b9e0117c2d4fa5510c3e8812'),
		svc(
			'migrate',
			'pull',
			'ghcr.io/acme/shop-web:2.4',
			'ghcr.io/acme/shop-web@sha256:1f4c2a9be07d3c55aa0021c1'
		),
		svc(
			'web',
			'pull',
			'ghcr.io/acme/shop-web:2.4',
			'ghcr.io/acme/shop-web@sha256:1f4c2a9be07d3c55aa0021c1'
		)
	];
}

const C2 = minutesAgo(90);
const C1 = minutesAgo(60 * 30);

export const MOCK_COMPOSE_DEPLOYMENTS: Record<string, Deployment[]> = {
	app_compose: [
		{
			id: 'dep_c2',
			app_id: 'app_compose',
			image_tag: 'tond-shop-stack:9c1e0a7b2d4f',
			status: 'succeeded',
			trigger_source: 'manual',
			trigger_detail: 'admin@tond.local',
			has_generated_dockerfile: false,
			started_at: C2,
			finished_at: minutesAgo(90, 73),
			created_at: C2,
			duration_secs: 73,
			phases: phasesFrom(C2, COMPOSE_STEPS),
			failure: null,
			image_pruned: false,
			is_current: true,
			rollback_available: false,
			rollback_unavailable_reason: 'This image is already running',
			services: mockReleaseServices('9c1e0a7b2d4f')
		},
		{
			id: 'dep_c1',
			app_id: 'app_compose',
			image_tag: 'tond-shop-stack:5d2b8e1f0a93',
			status: 'succeeded',
			trigger_source: 'manual',
			trigger_detail: 'admin@tond.local',
			has_generated_dockerfile: false,
			started_at: C1,
			finished_at: minutesAgo(60 * 30, 81),
			created_at: C1,
			duration_secs: 81,
			phases: phasesFrom(C1, COMPOSE_STEPS),
			failure: null,
			image_pruned: false,
			is_current: false,
			rollback_available: true,
			rollback_unavailable_reason: null,
			services: mockReleaseServices('5d2b8e1f0a93')
		}
	]
};

/** Parse services out of a compose file, well enough for the mock. */
export function mockComposeSpecs(content: string) {
	const specs: {
		name: string;
		image: string | null;
		builds: boolean;
		ports: number[];
		has_healthcheck: boolean;
	}[] = [];
	let inServices = false;
	let current: (typeof specs)[number] | null = null;
	for (const raw of content.split('\n')) {
		if (!raw.trim() || raw.trim().startsWith('#')) continue;
		const indent = raw.length - raw.trimStart().length;
		const line = raw.trim();
		if (indent === 0) {
			inServices = line.startsWith('services:');
			current = null;
			continue;
		}
		if (!inServices) continue;
		const svc = /^([A-Za-z0-9][\w.-]*):\s*$/.exec(line);
		if (indent === 2 && svc) {
			current = { name: svc[1], image: null, builds: false, ports: [], has_healthcheck: false };
			specs.push(current);
			continue;
		}
		if (!current) continue;
		const image = /^image:\s*["']?([^"'\s]+)/.exec(line);
		if (image) current.image = image[1];
		if (/^build:/.test(line)) current.builds = true;
		if (/^healthcheck:/.test(line)) current.has_healthcheck = true;
		const inline = /^(?:ports|expose):\s*\[(.*)\]/.exec(line);
		for (const item of inline ? inline[1].split(',') : []) {
			const port = Number(
				item
					.replace(/["'\s]/g, '')
					.split(':')
					.pop()
					?.split('/')[0]
			);
			if (port > 0 && !current.ports.includes(port)) current.ports.push(port);
		}
		const listed = /^-\s*["']?([\d.:]+)["']?\s*$/.exec(line);
		if (listed) {
			const port = Number(listed[1].split(':').pop());
			if (port > 0 && !current.ports.includes(port)) current.ports.push(port);
		}
	}
	return specs;
}
