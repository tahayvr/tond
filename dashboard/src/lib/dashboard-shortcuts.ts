export interface DashboardShortcutState {
	terminalOpen: boolean;
}

export interface DashboardShortcutActions {
	setTerminalOpen: (value: boolean) => void;
}

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
	function handleKeydown(event: KeyboardEvent) {
		const { terminalOpen } = getState();

		if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey) {
			if (event.code !== 'Backquote') return;
			if (isTypingTarget(event.target) && !isTerminalTarget(event.target)) return;

			event.preventDefault();
			actions.setTerminalOpen(!terminalOpen);
			return;
		}

		if (event.key === 'Escape' && terminalOpen && !isTypingTarget(event.target)) {
			event.preventDefault();
			actions.setTerminalOpen(false);
		}
	}

	window.addEventListener('keydown', handleKeydown, { capture: true });
	return () => window.removeEventListener('keydown', handleKeydown, { capture: true });
}
