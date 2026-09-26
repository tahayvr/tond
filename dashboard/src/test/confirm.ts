// Test helpers for flows that go through ConfirmDialog / confirmAction().

import { render, screen, within } from '@testing-library/svelte';
import type { UserEvent } from '@testing-library/user-event';
import ConfirmHost from '$lib/components/dialogs/confirm-host.svelte';

/** Mount the host that renders confirmAction() requests (the root layout does this in the app). */
export function renderConfirmHost() {
	return render(ConfirmHost);
}

/** Wait for the open confirmation dialog, optionally type the confirmation text, and confirm. */
export async function confirmDialog(user: UserEvent, { typed }: { typed?: string } = {}) {
	const dialog = await screen.findByRole('alertdialog');
	if (typed !== undefined) {
		await user.type(within(dialog).getByRole('textbox'), typed);
	}
	const submit = dialog.querySelector<HTMLButtonElement>('button[type="submit"]');
	if (!submit) throw new Error('confirm dialog has no submit button');
	await user.click(submit);
	return dialog;
}

/** Cancel the open confirmation dialog. */
export async function cancelDialog(user: UserEvent) {
	const dialog = await screen.findByRole('alertdialog');
	await user.click(within(dialog).getByRole('button', { name: /cancel/i }));
}
