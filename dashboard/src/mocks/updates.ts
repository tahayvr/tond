/**
 * Mock API for System → Updates. Spread into `handlers` in ./handlers.ts.
 *
 * "Update now" plays a whole run in about 20 seconds: snapshot, download,
 * backup, install, migrate (the "server" answers 502 for a few seconds, as it
 * restarts), proxy, then success on the new version with a rollback offer.
 *
 * Scenarios (localStorage `tond:mock-update`):
 *   up-to-date   running the latest release, rollback available
 *   rolled-back  the last update failed and was rolled back
 *   failing      "Update now" fails at the migrate step and rolls back
 */

import { http, HttpResponse, delay } from 'msw';
import type { TondRelease, UpdateOverview, UpdateRollbackOption, UpdateRun } from '$lib/api';
import { errorBody, okBody } from './envelope';

const LATENCY = 150;
const SCENARIO_KEY = 'tond:mock-update';

function ok<T>(data: T) {
	return HttpResponse.json(okBody(data));
}

function err(message: string, status = 400) {
	return HttpResponse.json(errorBody(message, status), { status });
}

function scenario(): string | null {
	try {
		return globalThis.localStorage?.getItem(SCENARIO_KEY) ?? null;
	} catch {
		return null;
	}
}

const now = () => Math.floor(Date.now() / 1000);
const DAY = 86_400;
const MOCK_SNAPSHOT_ID = '5f2c9a1e-6b0d-4c55-9a7e-2d1b8f3c4e5a';

export const MOCK_RELEASES: TondRelease[] = [
	{
		version: '0.4.0',
		tag: 'v0.4.0',
		name: 'Tond 0.4.0',
		url: 'https://github.com/tahayvr/tond/releases/tag/v0.4.0',
		published_at: new Date((now() - 2 * DAY) * 1000).toISOString(),
		body: [
			'## Highlights',
			'',
			'- **Updates from the dashboard**: System → Updates takes a snapshot, installs, migrates and rolls back automatically if anything fails.',
			'- Debian 12/13 and Ubuntu 24.04/26.04 support matrix, checked by the installer.',
			'- Docs at [tond.io/docs](https://tond.io/docs), with `llms.txt` for agents.',
			'',
			'## Fixes',
			'',
			'- The proxy restarts in seconds, not minutes.',
			'- `tond apps logs | head` exits quietly.'
		].join('\n')
	},
	{
		version: '0.3.1',
		tag: 'v0.3.1',
		name: 'Tond 0.3.1',
		url: 'https://github.com/tahayvr/tond/releases/tag/v0.3.1',
		published_at: new Date((now() - 12 * DAY) * 1000).toISOString(),
		body: '## Fixes\n\n- Certificates renew 30 days before they expire.\n- Faster `tond apps deploy` output.'
	},
	{
		version: '0.3.0',
		tag: 'v0.3.0',
		name: 'Tond 0.3.0',
		url: 'https://github.com/tahayvr/tond/releases/tag/v0.3.0',
		published_at: new Date((now() - 40 * DAY) * 1000).toISOString(),
		body: 'Observability: metrics, uptime checks and alerts.'
	}
];

const cmp = (a: string, b: string) => {
	const pa = a.split('.').map(Number);
	const pb = b.split('.').map(Number);
	for (let i = 0; i < 3; i++) if (pa[i] !== pb[i]) return pa[i] - pb[i];
	return 0;
};

interface MockState {
	current: string;
	checkedAt: number;
	checkEnabled: boolean;
	run: UpdateRun | null;
	rollback: UpdateRollbackOption | null;
	/** Simulated run: when it started and how it ends. */
	sim: { startedAt: number; kind: 'update' | 'rollback'; fails: boolean; to: string } | null;
}

function initialState(): MockState {
	const s: MockState = {
		current: '0.3.0',
		checkedAt: now() - 3 * 3600,
		checkEnabled: true,
		run: null,
		rollback: null,
		sim: null
	};
	if (scenario() === 'up-to-date') {
		s.current = '0.4.0';
		s.rollback = {
			to_version: '0.3.0',
			backup_created_at: now() - DAY,
			available_until: now() + 6 * DAY,
			restores_database: true
		};
		s.run = finishedRun('update', 'succeeded', '0.3.0', '0.4.0', now() - DAY);
	}
	if (scenario() === 'rolled-back') {
		s.run = finishedRun('update', 'rolled_back', '0.3.0', '0.4.0', now() - 600);
	}
	return s;
}

function finishedRun(
	kind: 'update' | 'rollback',
	state: UpdateRun['state'],
	from: string,
	to: string,
	at: number
): UpdateRun {
	const rolled = state === 'rolled_back';
	return {
		id: `20260926T101500Z-${kind}`,
		kind,
		state,
		effective_state: state,
		step: 'done',
		from_version: from,
		to_version: to,
		actor: 'admin@example.com',
		snapshot_id: MOCK_SNAPSHOT_ID,
		started_at: at - 120,
		updated_at: at,
		finished_at: at,
		message: rolled
			? `The update to ${to} failed (tond-server ${to} did not become healthy within 90 s); Tond ${from} is running again. Details: journalctl -u tond-server -u tond-update`
			: `Updated Tond ${from} → ${to}. You can roll back for 7 days (System → Updates, or update.sh --rollback).`,
		pid: null,
		database_restored: rolled,
		log: rolled
			? [
					`[update] Update ${from} → ${to} requested by admin@example.com`,
					'[update] Snapshot 5f2c9a1e taken.',
					`▶ Installing Tond ${to}`,
					`▶ Starting Tond ${to} (database migrations run now)`,
					`▶ Putting Tond ${from} back: tond-server ${to} did not become healthy within 90 s`,
					'[update] Put the database back as it was before this run'
				]
			: [
					`[update] Update ${from} → ${to} requested by admin@example.com`,
					'[update] Snapshot 5f2c9a1e taken.',
					`[update] Updated Tond ${from} → ${to}.`
				]
	};
}

// The state survives page reloads (the panel reloads after a successful
// update to load the new dashboard), per browser tab.
const STATE_KEY = 'tond:mock-update-state';

function loadState(): MockState {
	try {
		const saved = globalThis.sessionStorage?.getItem(STATE_KEY);
		if (saved) {
			const parsed = JSON.parse(saved) as MockState & { scenario?: string | null };
			if (parsed.scenario === scenario()) return parsed;
		}
	} catch {
		// fall through
	}
	return initialState();
}

function saveState() {
	try {
		globalThis.sessionStorage?.setItem(
			STATE_KEY,
			JSON.stringify({ ...state, scenario: scenario() })
		);
	} catch {
		// ignore
	}
}

const state = loadState();

// Seconds into the run → [step, message, log line].
const UPDATE_TIMELINE: [number, string, string, string][] = [
	[0, 'snapshot', 'Taking a full snapshot', '[update] Taking a full snapshot first…'],
	[
		3,
		'download',
		'Downloading Tond {to}',
		'[update] Downloading https://github.com/tahayvr/tond/releases/download/v{to}/tond-linux-x86_64.tar.gz'
	],
	[5, 'download', 'Verifying Tond {to}', 'Checksum verified. Signature of v{to} verified.'],
	[
		6,
		'backup',
		'Backing up Tond {from}',
		'[update] Backed up Tond {from} to /opt/tond/data/backups/tond_backup_20260926_101512'
	],
	[8, 'install', 'Installing Tond {to}', '▶ Installing Tond {to}'],
	[
		10,
		'migrate',
		'Starting Tond {to} (database migrations run now)',
		'▶ Starting Tond {to} (database migrations run now)'
	],
	[16, 'proxy', 'Restarting tond-proxy', '[update] tond-server {to} is healthy'],
	[18, 'done', '', '▶ Restarting tond-proxy']
];
const ROLLBACK_TIMELINE: [number, string, string, string][] = [
	[0, 'snapshot', 'Taking a full snapshot', '[update] Taking a full snapshot first…'],
	[3, 'backup', 'Backing up Tond {from}', '[update] Rolling back Tond {from} → {to}'],
	[5, 'install', 'Installing Tond {to}', '▶ Installing Tond {to}'],
	[7, 'migrate', 'Starting Tond {to}', '▶ Starting Tond {to}'],
	[12, 'proxy', 'Starting tond-proxy', '▶ Starting tond-proxy'],
	[14, 'done', '', '[update] Done']
];
/** The "server" is down during these seconds of an update. */
const DOWN = { update: [11, 14], rollback: [8, 11] };

function fill(s: string, from: string, to: string) {
	return s.replaceAll('{from}', from).replaceAll('{to}', to);
}

/** Advance the simulated run to the current time. */
function tick(): 'down' | null {
	const sim = state.sim;
	const run = state.run;
	if (!sim || !run) return null;
	const elapsed = now() - sim.startedAt;
	const timeline = sim.kind === 'rollback' ? ROLLBACK_TIMELINE : UPDATE_TIMELINE;
	const reached = timeline.filter(([t]) => elapsed >= t);
	const [, step, message] = reached[reached.length - 1];
	const log = [
		`[update] ${sim.kind === 'rollback' ? 'Rollback' : 'Update'} ${run.from_version} → ${sim.to} requested by admin@example.com`,
		...reached.map(([, , , line]) => fill(line, run.from_version, sim.to))
	];
	if (sim.fails && step !== 'snapshot' && elapsed >= 13) {
		Object.assign(run, finishedRun(sim.kind, 'rolled_back', run.from_version, sim.to, now()), {
			id: run.id,
			started_at: run.started_at,
			log: [
				...log,
				`▶ Putting Tond ${run.from_version} back: tond-server ${sim.to} did not become healthy within 90 s`
			]
		});
		state.sim = null;
		return null;
	}
	if (step === 'done') {
		const from = run.from_version;
		Object.assign(run, finishedRun(sim.kind, 'succeeded', from, sim.to, now()), {
			id: run.id,
			started_at: run.started_at,
			message:
				sim.kind === 'rollback'
					? `Rolled back Tond ${from} → ${sim.to}.`
					: `Updated Tond ${from} → ${sim.to}. You can roll back for 7 days (System → Updates, or update.sh --rollback).`,
			log: [
				...log,
				sim.kind === 'rollback'
					? `[update] Rolled back.`
					: `[update] Updated Tond ${from} → ${sim.to}.`
			]
		});
		state.current = sim.to;
		state.rollback =
			sim.kind === 'update'
				? {
						to_version: from,
						backup_created_at: sim.startedAt,
						available_until: sim.startedAt + 7 * DAY,
						restores_database: true
					}
				: null;
		state.sim = null;
		return null;
	}
	run.step = step;
	if (step !== 'snapshot') run.snapshot_id ??= MOCK_SNAPSHOT_ID;
	run.message = fill(message, run.from_version, sim.to);
	run.updated_at = now();
	run.log = log;
	const [downFrom, downTo] = DOWN[sim.kind];
	return elapsed >= downFrom && elapsed < downTo ? 'down' : null;
}

function overview(): UpdateOverview {
	const newer = MOCK_RELEASES.filter((r) => cmp(r.version, state.current) > 0);
	return {
		current_version: state.current,
		schema_version: 60,
		latest: MOCK_RELEASES[0],
		update_available: newer.length > 0,
		changelog: newer,
		check: { enabled: state.checkEnabled, checked_at: state.checkedAt, error: null },
		run: state.run ? { ...state.run } : null,
		rollback: state.run?.effective_state === 'running' ? null : state.rollback,
		unsupported_reason: null
	};
}

function startRun(kind: 'update' | 'rollback', to: string): UpdateRun {
	const from = state.current;
	state.sim = { startedAt: now(), kind, fails: scenario() === 'failing' && kind === 'update', to };
	state.run = {
		id: `${new Date().toISOString().replace(/[-:]/g, '').slice(0, 15)}Z-${Math.random().toString(16).slice(2, 10)}`,
		kind,
		state: 'running',
		effective_state: 'running',
		step: 'snapshot',
		from_version: from,
		to_version: to,
		actor: 'admin@example.com',
		snapshot_id: null,
		started_at: now(),
		updated_at: now(),
		finished_at: null,
		message: 'Taking a full snapshot',
		pid: 4242,
		database_restored: false,
		log: [
			`[update] ${kind === 'rollback' ? 'Rollback' : 'Update'} ${from} → ${to} requested by admin@example.com`
		]
	};
	return { ...state.run };
}

const running = () => state.run?.effective_state === 'running';

export const updateHandlers = [
	http.get('/api/v1/system/update', async () => {
		await delay(LATENCY);
		const down = tick();
		saveState();
		if (down === 'down') {
			return new HttpResponse('<html><body><h1>502 Bad Gateway</h1></body></html>', {
				status: 502,
				headers: { 'Content-Type': 'text/html' }
			});
		}
		return ok(overview());
	}),

	http.post('/api/v1/system/update/check', async () => {
		await delay(900);
		state.checkedAt = now();
		saveState();
		return ok(overview());
	}),

	http.put('/api/v1/system/update/settings', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { check_enabled?: boolean };
		state.checkEnabled = body.check_enabled !== false;
		saveState();
		return ok(overview());
	}),

	http.post('/api/v1/system/update', async ({ request }) => {
		await delay(LATENCY);
		if (running())
			return err('An update or rollback is already running. Wait for it to finish.', 409);
		const body = (await request.json().catch(() => ({}))) as { version?: string };
		const target = body.version ?? MOCK_RELEASES[0].version;
		if (cmp(target, state.current) <= 0) return err(`Tond ${state.current} is already running.`);
		const started = startRun('update', target);
		saveState();
		return HttpResponse.json(okBody(started), { status: 202 });
	}),

	http.post('/api/v1/system/update/rollback', async () => {
		await delay(LATENCY);
		if (running())
			return err('An update or rollback is already running. Wait for it to finish.', 409);
		if (!state.rollback)
			return err(
				'There is no previous version to roll back to: rollback is offered for 7 days after an update.'
			);
		const to = state.rollback.to_version;
		state.rollback = null;
		const started = startRun('rollback', to);
		saveState();
		return HttpResponse.json(okBody(started), { status: 202 });
	})
];
