import { describe, expect, it } from 'vitest';
import { ApiError, type UpdateRun } from '$lib/api';
import {
	formatDate,
	isRestarting,
	needsReload,
	parseInline,
	parseReleaseNotes,
	runSummary,
	stepStates
} from './updates';

function run(over: Partial<UpdateRun> = {}): UpdateRun {
	return {
		id: 'r1',
		kind: 'update',
		state: 'running',
		effective_state: 'running',
		step: 'install',
		from_version: '0.3.0',
		to_version: '0.4.0',
		actor: 'admin@example.com',
		snapshot_id: 'snap',
		started_at: 1,
		updated_at: 2,
		finished_at: null,
		message: 'Installing Tond 0.4.0',
		pid: 1,
		database_restored: false,
		log: [],
		...over
	};
}

describe('stepStates', () => {
	it('marks steps before the current one done and the rest pending', () => {
		const states = stepStates(run()).map((s) => `${s.step.id}:${s.state}`);
		expect(states).toEqual([
			'snapshot:done',
			'download:done',
			'backup:done',
			'install:current',
			'migrate:pending',
			'proxy:pending'
		]);
	});

	it('shows a skipped snapshot and uses the rollback steps for rollbacks', () => {
		const skipped = stepStates(run({ snapshot_id: null }));
		expect(skipped[0].state).toBe('skipped');
		const rb = stepStates(run({ kind: 'rollback', step: 'migrate' })).map((s) => s.step.label);
		expect(rb).toEqual(['Snapshot', 'Back up', 'Install', 'Start', 'Start proxy']);
	});

	it('is all done once the run succeeded', () => {
		const done = stepStates(
			run({ state: 'succeeded', effective_state: 'succeeded', step: 'done' })
		);
		expect(done.every((s) => s.state === 'done')).toBe(true);
	});
});

describe('runSummary', () => {
	it('describes running, failing-over and finished runs', () => {
		expect(runSummary(run()).title).toBe('Updating Tond 0.3.0 → 0.4.0');
		expect(runSummary(run({ step: 'rollback' })).tone).toBe('warning');
		expect(
			runSummary(run({ kind: 'rollback', to_version: '0.3.0', from_version: '0.4.0' })).title
		).toBe('Rolling back to Tond 0.3.0');
		const ok = runSummary(run({ state: 'succeeded', effective_state: 'succeeded' }));
		expect(ok).toMatchObject({ tone: 'success', title: 'Updated to Tond 0.4.0' });
	});

	it('explains an automatic rollback and whether the database came back', () => {
		const s = runSummary(
			run({
				state: 'rolled_back',
				effective_state: 'rolled_back',
				database_restored: true,
				message: 'tond-server 0.4.0 did not become healthy'
			})
		);
		expect(s.tone).toBe('warning');
		expect(s.title).toBe('The update to 0.4.0 was rolled back');
		expect(s.detail).toContain('healthy. The database');
		expect(s.detail).toContain('did not become healthy');
		expect(s.detail).toContain('Nothing was lost');
	});

	it('flags interrupted and failed runs as errors', () => {
		expect(runSummary(run({ effective_state: 'interrupted' })).tone).toBe('danger');
		const failed = runSummary(
			run({
				state: 'failed',
				effective_state: 'failed',
				message: 'Checksum mismatch. Nothing was changed.'
			})
		);
		expect(failed).toMatchObject({ tone: 'danger', title: "The update to 0.4.0 didn't happen" });
	});
});

describe('isRestarting and needsReload', () => {
	it('treats unreachable and gateway errors as a restart', () => {
		expect(isRestarting(new ApiError('down', 0))).toBe(true);
		expect(isRestarting(new ApiError('bad gateway', 502))).toBe(true);
		expect(isRestarting(new ApiError('forbidden', 403))).toBe(false);
		expect(isRestarting(new Error('x'))).toBe(false);
	});

	it('reloads only after a successful run changed the version', () => {
		const ok = run({ state: 'succeeded', effective_state: 'succeeded' });
		expect(needsReload('0.3.0', '0.4.0', ok)).toBe(true);
		expect(needsReload('0.4.0', '0.4.0', ok)).toBe(false);
		expect(needsReload('0.3.0', '0.4.0', run())).toBe(false);
		expect(needsReload(null, '0.4.0', ok)).toBe(false);
	});
});

describe('release notes', () => {
	it('parses headings, lists, paragraphs and code blocks', () => {
		const blocks = parseReleaseNotes(
			'## Highlights\n\n- one\n- two\n  continued\n\n1. first\n\nSome text\nwrapped.\n\n```\ncode\n```'
		);
		expect(blocks.map((b) => b.type)).toEqual(['heading', 'list', 'list', 'paragraph', 'code']);
		const list = blocks[1];
		expect(list.type === 'list' && list.items.length).toBe(2);
		expect(list.type === 'list' && list.items[1]).toEqual([
			{ type: 'text', text: 'two continued' }
		]);
		expect(blocks[2].type === 'list' && blocks[2].ordered).toBe(true);
		expect(blocks[3]).toEqual({
			type: 'paragraph',
			content: [{ type: 'text', text: 'Some text wrapped.' }]
		});
	});

	it('keeps only web links and never passes markup through', () => {
		expect(parseInline('see [docs](https://tond.io/docs) and `x`')).toEqual([
			{ type: 'text', text: 'see ' },
			{ type: 'link', text: 'docs', href: 'https://tond.io/docs' },
			{ type: 'text', text: ' and ' },
			{ type: 'code', text: 'x' }
		]);
		expect(parseInline('[bad](javascript:alert(1))')[0]).toEqual({ type: 'text', text: 'bad' });
		expect(parseInline('<img src=x onerror=alert(1)>')).toEqual([
			{ type: 'text', text: '<img src=x onerror=alert(1)>' }
		]);
		expect(parseInline('**bold**')).toEqual([{ type: 'strong', text: 'bold' }]);
	});
});

describe('formatDate', () => {
	it('formats ISO dates and Unix seconds, and tolerates nothing', () => {
		expect(formatDate('2026-09-20T10:00:00Z')).toBe('Sep 20, 2026');
		expect(formatDate(1_790_000_000)).toMatch(/2026/);
		expect(formatDate(null)).toBe('');
		expect(formatDate('garbage')).toBe('');
	});
});
