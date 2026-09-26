// System → Updates: presentation logic for update and rollback runs, and a
// small, safe renderer for release notes (Markdown from GitHub releases:
// headings, lists, paragraphs, code, bold and links; everything else is text,
// nothing is ever injected as HTML).

import { ApiError, type UpdateRun, type UpdateRunState } from '$lib/api';

export interface RunStep {
	id: string;
	label: string;
}

export const UPDATE_STEPS: readonly RunStep[] = [
	{ id: 'snapshot', label: 'Snapshot' },
	{ id: 'download', label: 'Download and verify' },
	{ id: 'backup', label: 'Back up' },
	{ id: 'install', label: 'Install' },
	{ id: 'migrate', label: 'Start and migrate' },
	{ id: 'proxy', label: 'Restart proxy' }
];

export const ROLLBACK_STEPS: readonly RunStep[] = [
	{ id: 'snapshot', label: 'Snapshot' },
	{ id: 'backup', label: 'Back up' },
	{ id: 'install', label: 'Install' },
	{ id: 'migrate', label: 'Start' },
	{ id: 'proxy', label: 'Start proxy' }
];

export type StepState = 'done' | 'current' | 'pending' | 'skipped';

/** Each step of a running run, and where it stands. */
export function stepStates(run: UpdateRun): { step: RunStep; state: StepState }[] {
	const steps = run.kind === 'rollback' ? ROLLBACK_STEPS : UPDATE_STEPS;
	const finished = run.effective_state === 'succeeded';
	let current = steps.findIndex((s) => s.id === run.step);
	if (finished || run.step === 'done') current = steps.length;
	return steps.map((step, i) => {
		let state: StepState = i < current ? 'done' : i === current ? 'current' : 'pending';
		// The snapshot step was skipped when the run moved past it without one.
		if (step.id === 'snapshot' && state === 'done' && !run.snapshot_id) state = 'skipped';
		return { step, state };
	});
}

export type Tone = 'info' | 'success' | 'warning' | 'danger';

export interface RunSummary {
	tone: Tone;
	title: string;
	detail: string;
}

/** Headline and explanation for a run (running or finished). */
export function runSummary(run: UpdateRun): RunSummary {
	const { from_version: from, to_version: to } = run;
	const rollback = run.kind === 'rollback';
	const state: UpdateRunState = run.effective_state;
	switch (state) {
		case 'running':
			if (run.step === 'rollback') {
				return {
					tone: 'warning',
					title: rollback
						? `Putting Tond ${from} back`
						: `The update failed: putting Tond ${from} back`,
					detail: run.message
				};
			}
			return {
				tone: 'info',
				title: rollback ? `Rolling back to Tond ${to}` : `Updating Tond ${from} → ${to}`,
				detail: run.message
			};
		case 'succeeded':
			return {
				tone: 'success',
				title: rollback
					? `Rolled back to Tond ${to}`
					: from === to
						? `Reinstalled Tond ${to}`
						: `Updated to Tond ${to}`,
				detail: run.message
			};
		case 'rolled_back':
			return {
				tone: 'warning',
				title: `The update to ${to} was rolled back`,
				detail: [
					sentence(run.message),
					run.database_restored
						? 'The database from before the update was put back. Nothing was lost: Tond was stopped the whole time.'
						: ''
				]
					.filter(Boolean)
					.join(' ')
			};
		case 'interrupted':
			return {
				tone: 'danger',
				title: rollback ? 'The rollback stopped unexpectedly' : 'The update stopped unexpectedly',
				detail: `${run.message ? `${run.message}. ` : ''}The process doing the work is gone (the server may have rebooted). Check the log below and \`journalctl -u tond-update\` on the server.`
			};
		case 'failed':
		default:
			return {
				tone: 'danger',
				title: rollback ? `The rollback to ${to} failed` : `The update to ${to} didn't happen`,
				detail: run.message
			};
	}
}

/** Text ending in a full stop, so sentences can be joined. */
function sentence(text: string): string {
	const t = text.trim();
	return t === '' || /[.!?]$/.test(t) ? t : `${t}.`;
}

/** The request failed because the server is restarting (or briefly unreachable). */
export function isRestarting(e: unknown): boolean {
	return (
		e instanceof ApiError &&
		(e.status === 0 || e.status === 502 || e.status === 503 || e.status === 504)
	);
}

/** After a successful run the page must load the new dashboard's files. */
export function needsReload(
	loadedVersion: string | null,
	currentVersion: string,
	run: UpdateRun | null
): boolean {
	return (
		loadedVersion !== null &&
		loadedVersion !== currentVersion &&
		run !== null &&
		run.effective_state === 'succeeded'
	);
}

/** "Sep 20, 2026" from an ISO date or Unix seconds. */
export function formatDate(value: string | number | null | undefined): string {
	if (value === null || value === undefined || value === '') return '';
	const date = typeof value === 'number' ? new Date(value * 1000) : new Date(value);
	if (Number.isNaN(date.getTime())) return '';
	return date.toLocaleDateString('en-US', { year: 'numeric', month: 'short', day: 'numeric' });
}

// ── Release notes ──

export type Inline =
	| { type: 'text'; text: string }
	| { type: 'code'; text: string }
	| { type: 'strong'; text: string }
	| { type: 'link'; text: string; href: string };

export type Block =
	| { type: 'heading'; level: number; content: Inline[] }
	| { type: 'paragraph'; content: Inline[] }
	| { type: 'list'; ordered: boolean; items: Inline[][] }
	| { type: 'code'; text: string };

const INLINE = /(`[^`]+`)|(\*\*[^*]+\*\*|__[^_]+__)|(\[[^\]]+\]\([^)\s]+\))/g;

/** Inline Markdown: `code`, **bold**, [links](https://…); the rest is text. */
export function parseInline(text: string): Inline[] {
	const out: Inline[] = [];
	let last = 0;
	for (const m of text.matchAll(INLINE)) {
		const index = m.index ?? 0;
		if (index > last) out.push({ type: 'text', text: text.slice(last, index) });
		const token = m[0];
		if (m[1]) out.push({ type: 'code', text: token.slice(1, -1) });
		else if (m[2]) out.push({ type: 'strong', text: token.slice(2, -2) });
		else {
			const label = token.slice(1, token.indexOf(']'));
			const href = token.slice(token.indexOf('](') + 2, -1);
			// Only web links; anything else (javascript:, data:, relative) stays text.
			if (/^https?:\/\//i.test(href)) out.push({ type: 'link', text: label, href });
			else out.push({ type: 'text', text: label });
		}
		last = index + token.length;
	}
	if (last < text.length) out.push({ type: 'text', text: text.slice(last) });
	return out;
}

/** Block-level Markdown, the subset release notes use. */
export function parseReleaseNotes(markdown: string): Block[] {
	const blocks: Block[] = [];
	const lines = markdown.replace(/\r\n/g, '\n').split('\n');
	let paragraph: string[] = [];
	let list: { ordered: boolean; items: string[] } | null = null;

	const flushParagraph = () => {
		if (paragraph.length) {
			blocks.push({ type: 'paragraph', content: parseInline(paragraph.join(' ')) });
			paragraph = [];
		}
	};
	const flushList = () => {
		if (list) {
			blocks.push({ type: 'list', ordered: list.ordered, items: list.items.map(parseInline) });
			list = null;
		}
	};

	for (let i = 0; i < lines.length; i++) {
		const line = lines[i];
		const trimmed = line.trim();
		if (trimmed.startsWith('```')) {
			flushParagraph();
			flushList();
			const code: string[] = [];
			i++;
			while (i < lines.length && !lines[i].trim().startsWith('```')) code.push(lines[i++]);
			blocks.push({ type: 'code', text: code.join('\n') });
			continue;
		}
		if (trimmed === '') {
			flushParagraph();
			flushList();
			continue;
		}
		const heading = /^(#{1,6})\s+(.*?)\s*#*$/.exec(trimmed);
		if (heading) {
			flushParagraph();
			flushList();
			blocks.push({
				type: 'heading',
				level: heading[1].length,
				content: parseInline(heading[2])
			});
			continue;
		}
		const bullet = /^[-*+]\s+(.*)$/.exec(trimmed);
		const numbered = /^\d+[.)]\s+(.*)$/.exec(trimmed);
		if (bullet || numbered) {
			flushParagraph();
			const ordered = Boolean(numbered);
			if (list && list.ordered !== ordered) flushList();
			list ??= { ordered, items: [] };
			list.items.push((bullet ?? numbered)![1]);
			continue;
		}
		if (list && /^\s{2,}\S/.test(line)) {
			// Continuation of the previous list item.
			list.items[list.items.length - 1] += ` ${trimmed}`;
			continue;
		}
		flushList();
		paragraph.push(trimmed);
	}
	flushParagraph();
	flushList();
	return blocks;
}
