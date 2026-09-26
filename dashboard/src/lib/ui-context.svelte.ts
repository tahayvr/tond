import type { App, CronJob, ManagedDatabase, Project } from './api';

// ── Feed Descriptor ──────────────────────────────────────────────────────────
//
// Describes what the feed panel should show. Embedded in every UiContext so
// the feed updates automatically when context changes.
//
//   title      — displayed in the feed panel header
//   wsUrl      — WebSocket endpoint to stream logs from, or null for no stream
//   label      — short secondary label shown next to the title (optional)

export interface FeedDescriptor {
	title: string;
	wsUrl: string | null;
	label?: string;
}

// ── UI Context ──────────────────────────────────────────────────────────────
//
// A discriminated union of all possible contexts in the app.
// Each member carries a `feed` descriptor so the feed panel always knows
// what to display without a separate store.
//
// To add a new context:
//   1. Add a union member here with a `feed` field.
//   2. Call setUiContext() from the relevant page/component.
//
// Contexts:
//   idle        — no page loaded yet; feed shows nothing
//   projects    — homepage (no card selected); feed shows system log
//   project     — a project card is selected; feed shows project app logs
//   app         — an individual app detail page; feed shows that app's log stream
//   databases   — databases list page; feed shows all-databases container log stream
//   database    — a managed database detail page; feed shows container log stream
//   cron        — a cron job detail page; feed shows latest run output stream
//   alerts      — alerts list/detail page; no log stream (monitoring rules only)

export type UiContext =
	| { type: 'idle'; feed: FeedDescriptor }
	| { type: 'projects'; feed: FeedDescriptor }
	| { type: 'project'; project: Project; feed: FeedDescriptor }
	| { type: 'app'; app: App; feed: FeedDescriptor }
	| { type: 'databases'; feed: FeedDescriptor }
	| { type: 'database'; database: ManagedDatabase; feed: FeedDescriptor }
	| { type: 'cron'; cronJob: CronJob; feed: FeedDescriptor }
	| { type: 'alerts'; feed: FeedDescriptor }
	| { type: 'integrations'; feed: FeedDescriptor }
	| { type: 'snapshots'; feed: FeedDescriptor };

export type TerminalTargetKind = 'server' | 'app' | 'database';

export interface TerminalTarget {
	id: string;
	kind: TerminalTargetKind;
	label: string;
	shell_user: string | null;
	/** False for the host shell while it is switched off on the server. */
	enabled: boolean;
	/** Opening a session needs the password (and 2FA code). */
	requires_step_up: boolean;
}

interface TerminalState {
	open: boolean;
	selectedTargetId: string | null;
	selectedTargetKind: TerminalTargetKind | null;
	manualSelection: boolean;
}

let _context = $state<UiContext>({
	type: 'idle',
	feed: { title: 'Feed', wsUrl: null }
});

// ── Feed panel open state ─────────────────────────────────────────────────────
// Allows any component to imperatively expand the feed panel sidebar.
// +layout.svelte binds Sidebar.Provider open to this via getFeedOpen/setFeedOpen.

let _feedOpen = $state(false);
let _terminalOpen = $state(false);
let _terminalSelectedTargetId = $state<string | null>(null);
let _terminalSelectedTargetKind = $state<TerminalTargetKind | null>(null);
let _terminalManualSelection = $state(false);

export function getFeedOpen(): boolean {
	return _feedOpen;
}

export function setFeedOpen(value: boolean) {
	_feedOpen = value;
}

/** Expands the feed panel sidebar. Call this before/after triggering an operation. */
export function openFeedPanel() {
	_feedOpen = true;
}

// ── Command palette and shortcuts overlay ────────────────────────────────────

let _paletteOpen = $state(false);
let _shortcutsOpen = $state(false);

export function getPaletteOpen(): boolean {
	return _paletteOpen;
}

export function setPaletteOpen(value: boolean) {
	_paletteOpen = value;
	if (value) _shortcutsOpen = false;
}

export function getShortcutsOpen(): boolean {
	return _shortcutsOpen;
}

export function setShortcutsOpen(value: boolean) {
	_shortcutsOpen = value;
	if (value) _paletteOpen = false;
}

export function getTerminalState(): TerminalState {
	return {
		open: _terminalOpen,
		selectedTargetId: _terminalSelectedTargetId,
		selectedTargetKind: _terminalSelectedTargetKind,
		manualSelection: _terminalManualSelection
	};
}

export function getTerminalOpen(): boolean {
	return _terminalOpen;
}

export function setTerminalOpen(value: boolean) {
	_terminalOpen = value;
	if (value) {
		_feedOpen = false;
	}
	if (!value) {
		_terminalManualSelection = false;
	}
}

export function toggleTerminalOpen() {
	setTerminalOpen(!_terminalOpen);
}

export function setTerminalTarget(target: Pick<TerminalTarget, 'id' | 'kind'> | null) {
	_terminalSelectedTargetId = target?.id ?? null;
	_terminalSelectedTargetKind = target?.kind ?? null;
	_terminalManualSelection = target !== null;
}

export function syncTerminalTarget(targets: TerminalTarget[]) {
	const active =
		_terminalSelectedTargetId !== null && _terminalSelectedTargetKind !== null
			? targets.find(
					(target) =>
						target.id === _terminalSelectedTargetId && target.kind === _terminalSelectedTargetKind
				)
			: null;

	if (active) {
		return active;
	}

	const preferred = getPreferredTerminalTarget(targets, _context);
	_terminalSelectedTargetId = preferred?.id ?? null;
	_terminalSelectedTargetKind = preferred?.kind ?? null;
	if (!preferred) {
		_terminalManualSelection = false;
	}
	return preferred ?? null;
}

export function getPreferredTerminalTarget(
	targets: TerminalTarget[],
	ctx: UiContext = _context
): TerminalTarget | null {
	if (ctx.type === 'app') {
		const appTarget = targets.find((target) => target.kind === 'app' && target.id === ctx.app.id);
		if (appTarget) return appTarget;
	}

	if (ctx.type === 'database') {
		const dbTarget = targets.find(
			(target) => target.kind === 'database' && target.id === ctx.database.id
		);
		if (dbTarget) return dbTarget;
	}

	// Prefer the host shell when it's switched on, otherwise the first usable
	// container; fall back to the (switched-off) server so its notice explains why.
	return (
		targets.find((target) => target.kind === 'server' && target.enabled !== false) ??
		targets.find((target) => target.enabled !== false) ??
		targets.find((target) => target.kind === 'server') ??
		targets[0] ??
		null
	);
}

export function getUiContext(): UiContext {
	return _context;
}

export function setUiContext(ctx: UiContext) {
	_context = ctx;
}

export function clearUiContext() {
	_context = { type: 'idle', feed: { title: 'Feed', wsUrl: null } };
}

// ── Convenience constructors ─────────────────────────────────────────────────
// Use these to build a context with the correct feed descriptor already set.

export function projectsContext(): UiContext {
	return {
		type: 'projects',
		feed: { title: 'System_Log', wsUrl: '/api/v1/system/logs/stream', label: '' }
	};
}

export function databasesContext(): UiContext {
	return {
		type: 'databases',
		feed: { title: 'DB_Log', wsUrl: '/api/v1/databases/logs/stream', label: '' }
	};
}

export function projectContext(project: Project): UiContext {
	return {
		type: 'project',
		project,
		feed: { title: project.name, wsUrl: null, label: 'select an app for logs' }
	};
}

export function appContext(app: App): UiContext {
	return {
		type: 'app',
		app,
		feed: {
			title: app.name,
			wsUrl: `/api/v1/apps/${app.id}/logs/stream`,
			label: 'live stream'
		}
	};
}

export function databaseContext(database: ManagedDatabase): UiContext {
	return {
		type: 'database',
		database,
		feed: {
			title: database.name,
			wsUrl: `/api/v1/databases/${database.id}/logs/stream`,
			label: 'container'
		}
	};
}

export function cronJobContext(cronJob: CronJob): UiContext {
	return {
		type: 'cron',
		cronJob,
		feed: {
			title: cronJob.name,
			wsUrl: `/api/v1/cron/${cronJob.id}/runs/stream`,
			label: 'last run'
		}
	};
}

export function alertsContext(): UiContext {
	return {
		type: 'alerts',
		feed: { title: 'Alerts', wsUrl: null, label: '' }
	};
}

export function integrationsContext(): UiContext {
	return {
		type: 'integrations',
		feed: { title: 'Integrations', wsUrl: null, label: '' }
	};
}

export function snapshotsContext(): UiContext {
	return {
		type: 'snapshots',
		feed: { title: 'Snapshots', wsUrl: null, label: '' }
	};
}
