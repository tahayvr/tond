// Imperative confirmations, rendered by the single <ConfirmHost /> in the root layout.
//
//   const ok = await confirmAction({
//     title: 'Delete backup job?',
//     description: 'Scheduled backups stop. Existing snapshots are kept.',
//     confirmLabel: 'Delete job',
//     onconfirm: () => deleteCronJob(id)
//   });
//
// `onconfirm` runs inside the dialog (pending label, inline error, can't be
// dismissed mid-flight). Resolves true once it succeeds, false on cancel.

export interface ConfirmOptions {
	title: string;
	description?: string;
	/** Bullet points spelling out exactly what is removed or overwritten. */
	details?: string[];
	confirmLabel?: string;
	pendingLabel?: string;
	destructive?: boolean;
	/** Require typing this text (irreversible data loss only). */
	typedConfirmation?: string;
	onconfirm?: () => unknown;
}

interface PendingConfirm extends ConfirmOptions {
	id: number;
	resolve: (confirmed: boolean) => void;
}

let current = $state<PendingConfirm | null>(null);
let nextId = 1;

export function getPendingConfirm(): PendingConfirm | null {
	return current;
}

export function confirmAction(options: ConfirmOptions): Promise<boolean> {
	// A new request replaces an unanswered one (treated as cancelled).
	current?.resolve(false);
	return new Promise((resolve) => {
		current = { ...options, id: nextId++, resolve };
	});
}

/** Called by ConfirmHost when the dialog closes. */
export function settleConfirm(id: number, confirmed: boolean) {
	if (!current || current.id !== id) return;
	const { resolve } = current;
	current = null;
	resolve(confirmed);
}
