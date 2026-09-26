/**
 * Mock API for host history, disk, Docker health, the heartbeat and uptime
 * checks (Phase 2 · Observable). Spread into `handlers` in ./handlers.ts.
 */

import { http, HttpResponse, delay } from 'msw';
import type {
	CleanupPreview,
	DiskUsage,
	HeartbeatStatus,
	HeartbeatUpdate,
	HostHistory,
	HostOverview,
	MetricsRange,
	UptimeBar,
	UptimeCheck,
	UptimeCheckUpdate,
	UptimeIncident,
	UptimeSummary
} from '$lib/api';
import { MOCK_APPS } from './fixtures';

const LATENCY = 200;
const GB = 1024 * 1024 * 1024;
const MB = 1024 * 1024;

function ok<T>(data: T) {
	return HttpResponse.json({ success: true, data, error: null });
}

function err(message: string, status = 400) {
	return HttpResponse.json({ success: false, data: null, error: message }, { status });
}

const now = () => Math.floor(Date.now() / 1000);

// ── Host ──

let buildCache = { bytes: 1.34 * GB, reclaimable: 0.91 * GB, count: 58 };
let dangling = [
	{ id: '3f9a2c1b7d44', bytes: 412 * MB, created: now() - 9 * 86_400 },
	{ id: 'a81be0c95f02', bytes: 287 * MB, created: now() - 5 * 86_400 },
	{ id: '0c77d1e2a9b3', bytes: 96 * MB, created: now() - 2 * 86_400 }
];

function mockDisk(): DiskUsage {
	return {
		images: { bytes: 5.2 * GB, count: 17, reclaimable: 3.4 * GB },
		containers: { bytes: 48 * MB, count: 9, reclaimable: 0 },
		volumes: { bytes: 0, count: 1, reclaimable: 0 },
		build_cache: {
			bytes: buildCache.bytes,
			count: buildCache.count,
			reclaimable: buildCache.reclaimable
		},
		tond_data: 6.4 * GB,
		data_dir: '/opt/tond/data',
		docker_root: '/var/lib/docker',
		docker_on_data_fs: true,
		error: null,
		collected_at: now() - 11 * 60,
		took_ms: 1830
	};
}

export function makeMockHostOverview(): HostOverview {
	const used = 31.6 * GB;
	return {
		current: {
			cpu_percent: 23.4,
			load1: 0.62,
			load5: 0.48,
			load15: 0.41,
			cpus: 2,
			mem_used: 1.46 * GB,
			mem_total: 3.82 * GB,
			swap_used: 96 * MB,
			swap_total: 1 * GB,
			data_fs: {
				mount: '/',
				used,
				total: 75 * GB,
				inodes_used: 412_000,
				inodes_total: 4_915_200
			},
			root_fs: null,
			collected_at: now() - 12
		},
		forecast: {
			used,
			total: 75 * GB,
			day: {
				window: '24h',
				status: 'filling',
				growth_per_day: 1.9 * GB,
				eta_secs: Math.round((43.4 * GB) / (1.9 * GB)) * 86_400,
				samples: 1440
			},
			week: {
				window: '7d',
				status: 'filling',
				growth_per_day: 0.6 * GB,
				eta_secs: 72 * 86_400,
				samples: 168
			},
			eta_secs: Math.round((43.4 * GB) / (1.9 * GB)) * 86_400
		},
		disk: mockDisk(),
		docker: {
			reachable: true,
			version: '29.4.0',
			api_version: '1.53',
			storage_driver: 'overlayfs',
			root_dir: '/var/lib/docker',
			cgroup_version: '2',
			warnings: ['No swap limit support'],
			containers: 9,
			containers_running: 7,
			images: 17,
			rss_bytes: 78 * MB,
			uptime_secs: 12 * 86_400 + 3 * 3600,
			error: null,
			checked_at: now() - 20
		}
	};
}

const SPANS: Record<MetricsRange, [number, number]> = {
	'1h': [3600, 60],
	'24h': [86_400, 180],
	'7d': [7 * 86_400, 3600],
	'30d': [30 * 86_400, 7200]
};

export function makeMockHostHistory(range: MetricsRange): HostHistory {
	const [span, step] = SPANS[range] ?? SPANS['24h'];
	const to = now();
	const from = Math.floor((to - span) / step) * step;
	const t: number[] = [];
	for (let x = from; x <= to; x += step) t.push(x);
	const n = t.length;
	const wave = (i: number, period: number) => Math.sin((i / Math.max(1, n)) * Math.PI * period);
	const cpu = t.map((_, i) => Math.max(2, 18 + 12 * wave(i, 3) + 4 * wave(i, 29)));
	const series = {
		t,
		cpu,
		cpu_max: cpu.map((c) => Math.min(100, c * 1.6)),
		load1: cpu.map((c) => +(c / 40).toFixed(2)),
		load5: cpu.map((c) => +(c / 50).toFixed(2)),
		load15: cpu.map((c) => +(c / 60).toFixed(2)),
		mem_used: t.map((_, i) => Math.round((1.3 + 0.25 * wave(i, 3)) * GB)),
		mem_total: t.map(() => Math.round(3.82 * GB)),
		swap_used: t.map((_, i) => Math.round((60 + 40 * Math.abs(wave(i, 2))) * MB)),
		swap_total: t.map(() => GB),
		disk_used: t.map((_, i) => Math.round((29.5 + (2.1 * i) / Math.max(1, n)) * GB)),
		disk_total: t.map(() => 75 * GB),
		inodes_used: t.map((_, i) => 400_000 + Math.round((12_000 * i) / Math.max(1, n))),
		inodes_total: t.map(() => 4_915_200),
		root_used: [] as number[],
		root_total: [] as number[],
		root_inodes_used: [] as number[],
		root_inodes_total: [] as number[]
	};
	series.root_used = series.disk_used;
	series.root_total = series.disk_total;
	series.root_inodes_used = series.inodes_used;
	series.root_inodes_total = series.inodes_total;
	return { range, step, from, to, separate_root: false, data_mount: '/', series };
}

function mockCleanupPreview(): CleanupPreview {
	const danglingBytes = dangling.reduce((s, d) => s + d.bytes, 0);
	return {
		build_cache_bytes: buildCache.reclaimable,
		build_cache_entries: buildCache.reclaimable > 0 ? 41 : 0,
		dangling_images: dangling,
		dangling_bytes: danglingBytes,
		total_bytes: buildCache.reclaimable + danglingBytes
	};
}

// ── Heartbeat ──

let heartbeat: HeartbeatStatus = {
	configured: true,
	url_host: 'hc-ping.com',
	method: 'GET',
	interval_secs: 300,
	enabled: true,
	last_ok_at: now() - 140,
	last_failure_at: null,
	last_error: null,
	last_skipped_at: null,
	last_skip_reason: null,
	next_at: now() + 160,
	healthy: true,
	unhealthy_reason: null
};

// ── Uptime ──

interface MockCheck {
	id: number;
	appId: string | null;
	name: string;
	host: string | null;
	settings: UptimeCheckUpdate;
	status: UptimeCheck['status'];
	pausedReason: string | null;
	downSince: number | null;
	incidents: UptimeIncident[];
}

function hostOf(app: (typeof MOCK_APPS)[number]): string | null {
	return app.domain ?? null;
}

const checks: MockCheck[] = [
	{
		id: 1,
		appId: null,
		name: 'Dashboard',
		host: 'tond.example.com',
		settings: {
			enabled: true,
			path: '/health',
			interval_secs: 60,
			expected_status: null,
			timeout_secs: 10
		},
		status: 'up',
		pausedReason: null,
		downSince: null,
		incidents: []
	},
	...MOCK_APPS.filter((a) => hostOf(a)).map((a, i): MockCheck => {
		const down = a.status === 'unhealthy';
		const paused = a.status === 'stopped';
		const incidents: UptimeIncident[] = [];
		if (down) {
			incidents.push({
				id: 100 + i,
				check_id: 2 + i,
				started_at: now() - 14 * 60,
				ended_at: null,
				cause: 'http_5xx',
				detail:
					"HTTP 502: the proxy couldn't reach the app. Is it running and listening on its port?",
				duration_secs: 14 * 60
			});
		}
		incidents.push(
			{
				id: 200 + i,
				check_id: 2 + i,
				started_at: now() - 2 * 86_400 - 5400,
				ended_at: now() - 2 * 86_400 - 5220,
				cause: 'timeout',
				detail: 'The app did not answer within 10 s (the connection and TLS were fine).',
				duration_secs: 180
			},
			{
				id: 300 + i,
				check_id: 2 + i,
				started_at: now() - 11 * 86_400,
				ended_at: now() - 11 * 86_400 + 1260,
				cause: 'tls',
				detail:
					'The certificate for ' +
					(hostOf(a) ?? '') +
					' has expired. Check Settings → Domain for renewal errors.',
				duration_secs: 1260
			}
		);
		return {
			id: 2 + i,
			appId: a.id,
			name: a.name,
			host: hostOf(a),
			settings: {
				enabled: true,
				path: '/',
				interval_secs: 60,
				expected_status: null,
				timeout_secs: 10
			},
			status: paused ? 'paused' : down ? 'down' : 'up',
			pausedReason: paused ? 'The app is stopped' : null,
			downSince: down ? now() - 14 * 60 : null,
			incidents
		};
	})
];

function bars(count: number, width: number, check: MockCheck, perBar: number): UptimeBar[] {
	const end = Math.floor(now() / width) * width + width;
	const start = end - width * count;
	return Array.from({ length: count }, (_, i) => {
		const t = start + i * width;
		if (check.status === 'paused' && i > count - 6) return { t, probes: 0, up: 0 };
		let up = perBar;
		for (const inc of check.incidents) {
			const s = inc.started_at;
			const e = inc.ended_at ?? now();
			const overlap = Math.max(0, Math.min(e, t + width) - Math.max(s, t));
			up -= Math.round((overlap / width) * perBar);
		}
		return { t, probes: perBar, up: Math.max(0, up) };
	});
}

function describe(check: MockCheck): UptimeCheck {
	const recent = bars(48, 1800, check, 30);
	const daily = bars(90, 86_400, check, 1440).map((b, i) =>
		i < 20 ? { ...b, probes: 0, up: 0 } : b
	);
	const pct = (list: UptimeBar[]) => {
		const probes = list.reduce((s, b) => s + b.probes, 0);
		return probes ? (list.reduce((s, b) => s + b.up, 0) / probes) * 100 : null;
	};
	const t: number[] = [];
	const ms: number[] = [];
	for (let x = now() - 86_400; x < now(); x += 600) {
		t.push(Math.floor(x / 600) * 600);
		ms.push(Math.round(38 + 10 * Math.sin(x / 5000) + 4 * Math.sin(x / 900)));
	}
	const active = check.status !== 'paused';
	return {
		check: check.host
			? {
					id: check.id,
					...check.settings
				}
			: null,
		host: check.host,
		url: check.host ? `https://${check.host}${check.settings.path}` : null,
		status: check.settings.enabled ? check.status : 'paused',
		paused_reason: check.settings.enabled
			? check.pausedReason
			: 'Checks are turned off for this app',
		last: active
			? {
					at: now() - 23,
					ok: check.status === 'up',
					cause: check.status === 'down' ? 'http_5xx' : null,
					detail:
						check.status === 'down'
							? "HTTP 502: the proxy couldn't reach the app. Is it running and listening on its port?"
							: 'HTTP 200',
					status: check.status === 'down' ? 502 : 200,
					latency_ms: check.status === 'down' ? 12 : 41,
					ip: '203.0.113.10',
					dns: 'matches',
					cert_expires_at: now() + 61 * 86_400,
					cert_days_left: 61,
					dns_label: 'Points at this server'
				}
			: null,
		down_since: check.downSince,
		next_at: active ? now() + 37 : null,
		uptime: {
			day: pct(recent),
			week: pct(daily.slice(-7)),
			month: pct(daily.slice(-30)),
			day_probes: recent.reduce((s, b) => s + b.probes, 0)
		},
		recent,
		daily,
		latency: { t, ms },
		incidents: check.incidents
	};
}

function noCheck(): UptimeCheck {
	return {
		check: null,
		host: null,
		url: null,
		status: 'paused',
		paused_reason: 'The app has no domain yet. Uptime checks start once it has one.',
		last: null,
		down_since: null,
		next_at: null,
		uptime: { day: null, week: null, month: null, day_probes: 0 },
		recent: [],
		daily: [],
		latency: { t: [], ms: [] },
		incidents: []
	};
}

function findAppCheck(id: string): MockCheck | null | undefined {
	const app = MOCK_APPS.find((a) => a.id === id || a.name === id);
	if (!app) return undefined;
	return checks.find((c) => c.appId === app.id) ?? null;
}

function validate(body: UptimeCheckUpdate): string | null {
	if (!body.path.startsWith('/')) return 'The path must start with / (for example /health)';
	if (body.interval_secs < 60 || body.interval_secs > 3600 || body.interval_secs % 60)
		return 'The interval must be a whole number of minutes, from 1 to 60';
	if (body.expected_status !== null && (body.expected_status < 100 || body.expected_status > 599))
		return 'The expected status must be an HTTP status code (100–599)';
	if (body.timeout_secs < 1 || body.timeout_secs > 30)
		return 'The timeout must be between 1 and 30 seconds';
	return null;
}

export const observeHandlers = [
	http.get('/api/system/host', async () => {
		await delay(LATENCY);
		const overview = makeMockHostOverview();
		overview.disk = mockDisk();
		return ok(overview);
	}),

	http.get('/api/system/host/history', async ({ request }) => {
		await delay(LATENCY);
		const range = (new URL(request.url).searchParams.get('range') ?? '24h') as MetricsRange;
		if (!(range in SPANS)) return err(`range must be one of 1h, 24h, 7d, 30d (got '${range}')`);
		return ok(makeMockHostHistory(range));
	}),

	http.get('/api/system/host/forecast', async () => {
		await delay(LATENCY);
		return ok(makeMockHostOverview().forecast);
	}),

	http.post('/api/system/disk/refresh', async () => {
		await delay(1200);
		return ok({ ...mockDisk(), collected_at: now() });
	}),

	http.get('/api/system/disk/cleanup', async () => {
		await delay(900);
		return ok(mockCleanupPreview());
	}),

	http.post('/api/system/disk/cleanup', async () => {
		await delay(1500);
		const preview = mockCleanupPreview();
		const result = {
			build_cache_bytes: preview.build_cache_bytes,
			build_cache_entries: preview.build_cache_entries,
			images_deleted: preview.dangling_images.length,
			image_bytes: preview.dangling_bytes,
			total_bytes: preview.total_bytes
		};
		buildCache = {
			bytes: buildCache.bytes - buildCache.reclaimable,
			reclaimable: 0,
			count: 17
		};
		dangling = [];
		return ok(result);
	}),

	http.get('/api/system/heartbeat', async () => {
		await delay(LATENCY);
		return ok(heartbeat);
	}),

	http.put('/api/system/heartbeat', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as HeartbeatUpdate;
		if (body.url !== undefined && body.url !== '') {
			let host: string;
			try {
				const u = new URL(body.url);
				if (!['http:', 'https:'].includes(u.protocol)) throw new Error('scheme');
				host = u.hostname;
			} catch {
				return err('Enter the full heartbeat URL, starting with https://');
			}
			heartbeat = {
				...heartbeat,
				configured: true,
				url_host: host,
				last_ok_at: null,
				last_failure_at: null,
				last_error: null
			};
		} else if (body.url === '') {
			heartbeat = {
				...heartbeat,
				configured: false,
				url_host: null,
				last_ok_at: null,
				next_at: null
			};
		}
		if (body.interval_secs !== undefined) {
			if (body.interval_secs < 60 || body.interval_secs > 86_400)
				return err('The interval must be between 1 minute and 24 hours');
			heartbeat.interval_secs = body.interval_secs;
		}
		if (body.method) heartbeat.method = body.method;
		if (body.enabled !== undefined) heartbeat.enabled = body.enabled;
		heartbeat.next_at =
			heartbeat.configured && heartbeat.enabled ? now() + heartbeat.interval_secs : null;
		return ok(heartbeat);
	}),

	http.post('/api/system/heartbeat/test', async () => {
		await delay(700);
		if (!heartbeat.configured) return err('Add a heartbeat URL first, then send a test ping');
		heartbeat = { ...heartbeat, last_ok_at: now() };
		return ok({ outcome: 'sent', status: 200, heartbeat });
	}),

	http.get('/api/uptime', async () => {
		await delay(LATENCY);
		const rows: UptimeSummary[] = checks.map((c) => {
			const d = describe(c);
			return {
				check_id: c.id,
				kind: c.appId ? 'app' : 'dashboard',
				app_id: c.appId,
				name: c.name,
				host: c.host ?? '',
				status: d.status,
				uptime: d.uptime,
				last_at: d.last?.at ?? null,
				latency_ms: d.last?.latency_ms ?? null,
				down_since: d.down_since
			};
		});
		return ok(rows);
	}),

	http.get('/api/system/uptime', async () => {
		await delay(LATENCY);
		return ok(describe(checks[0]));
	}),

	http.put('/api/system/uptime', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as UptimeCheckUpdate;
		const problem = validate(body);
		if (problem) return err(problem);
		checks[0].settings = body;
		return ok(describe(checks[0]));
	}),

	http.post('/api/system/uptime/check', async () => {
		await delay(900);
		return ok(describe(checks[0]));
	}),

	http.get('/api/apps/:id/uptime', async ({ params }) => {
		await delay(LATENCY);
		const check = findAppCheck(params.id as string);
		if (check === undefined) return err('App not found', 404);
		return ok(check ? describe(check) : noCheck());
	}),

	http.put('/api/apps/:id/uptime', async ({ params, request }) => {
		await delay(LATENCY);
		const check = findAppCheck(params.id as string);
		if (check === undefined) return err('App not found', 404);
		if (!check) return err('This app has no domain, so it has no uptime check yet');
		const body = (await request.json()) as UptimeCheckUpdate;
		const problem = validate(body);
		if (problem) return err(problem);
		check.settings = body;
		return ok(describe(check));
	}),

	http.post('/api/apps/:id/uptime/check', async ({ params }) => {
		await delay(1100);
		const check = findAppCheck(params.id as string);
		if (check === undefined) return err('App not found', 404);
		if (!check) return err('There is nothing to check: the app has no domain');
		if (!check.settings.enabled || check.status === 'paused')
			return err(check.pausedReason ?? 'Checks are turned off for this app');
		return ok(describe(check));
	})
];
