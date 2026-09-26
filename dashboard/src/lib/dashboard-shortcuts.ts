// Global keyboard shortcuts. The `?` overlay renders SHORTCUTS, so keep the
// list and the handler below in sync.

export interface DashboardShortcutState {
	terminalOpen: boolean;
}

export interface DashboardShortcutActions {
	setTerminalOpen: (value: boolean) => void;
	openPalette?: () => void;
	openShortcuts?: () => void;
	navigate?: (href: string) => void;
}

export interface ShortcutHelp {
	keys: string[];
	description: string;
}

/** `g` then a letter jumps to a page. */
export const GO_TO: Record<string, { href: string; label: string }> = {
	h: { href: '/', label: 'Home' },
	p: { href: '/projects', label: 'Projects' },
	a: { href: '/apps', label: 'Apps' },
	d: { href: '/databases', label: 'Databases' },
	c: { href: '/cron', label: 'Cron jobs' },
	l: { href: '/alerts', label: 'Alerts' },
	i: { href: '/integrations', label: 'Integrations' },
	s: { href: '/settings', label: 'Settings' }
};

export const SHORTCUTS: ShortcutHelp[] = [
	{ keys: ['⌘', 'K'], description: 'Open the command palette (Ctrl+K on Windows/Linux)' },
	{ keys: ['?'], description: 'Show keyboard shortcuts' },
	{ keys: ['Ctrl', '`'], description: 'Open or close the terminal' },
	{ keys: ['Esc'], description: 'Close the terminal, dialog or palette' },
	{ keys: ['⌘', 'B'], description: 'Show or hide the log feed' },
	...Object.entries(GO_TO).map(([key, { label }]) => ({
		keys: ['g', key],
		description: `Go to ${label}`
	}))
];

const CHORD_TIMEOUT_MS = 1200;

function isTypingTarget(target: EventTarget | null): boolean {
	if (!(target instanceof HTMLElement)) return false;
	const tag = target.tagName;
	return target.isContentEditable || tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
}

function isTerminalTarget(target: EventTarget | null): boolean {
	return target instanceof HTMLElement && target.closest('.terminal-pane') !== null;
}

export function registerDashboardShortcuts(
	getState: () => DashboardShortcutState,
	actions: DashboardShortcutActions
): () => void {
	let chordStartedAt = 0;

	function handleKeydown(event: KeyboardEvent) {
		const { terminalOpen } = getState();
		const typing = isTypingTarget(event.target);

		if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey) {
			if (event.code === 'Backquote') {
				if (typing && !isTerminalTarget(event.target)) return;
				event.preventDefault();
				actions.setTerminalOpen(!terminalOpen);
				return;
			}
			if (event.key.toLowerCase() === 'k' && actions.openPalette && !isTerminalTarget(event.target)) {
				event.preventDefault();
				actions.openPalette();
			}
			return;
		}

		if (event.key === 'Escape' && terminalOpen && !typing) {
			event.preventDefault();
			actions.setTerminalOpen(false);
			return;
		}

		// Single-key shortcuts never fire while typing or inside the terminal.
		if (typing || isTerminalTarget(event.target) || event.altKey || event.ctrlKey || event.metaKey) {
			return;
		}

		if (event.key === '?' && actions.openShortcuts) {
			event.preventDefault();
			actions.openShortcuts();
			return;
		}

		const now = Date.now();
		if (chordStartedAt && now - chordStartedAt <= CHORD_TIMEOUT_MS) {
			chordStartedAt = 0;
			const target = GO_TO[event.key.toLowerCase()];
			if (target && actions.navigate) {
				event.preventDefault();
				actions.navigate(target.href);
			}
			return;
		}
		chordStartedAt = event.key === 'g' ? now : 0;
	}

	window.addEventListener('keydown', handleKeydown, { capture: true });
	return () => window.removeEventListener('keydown', handleKeydown, { capture: true });
}
