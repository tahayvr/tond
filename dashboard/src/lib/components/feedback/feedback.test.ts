import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import axe from 'axe-core';
import { describe, expect, it, vi } from 'vitest';
import InlineError from './inline-error.svelte';
import SuccessAlert from './success-alert.svelte';
import ConfirmDialog from '../dialogs/confirm-dialog.svelte';
import { confirmAction } from '$lib/confirm.svelte';
import { confirmDialog, renderConfirmHost, cancelDialog } from '../../../test/confirm';

async function expectNoAxeViolations() {
	// Colour contrast is covered by src/test/contrast.test.ts (jsdom has no layout).
	const result = await axe.run(document.body, { rules: { 'color-contrast': { enabled: false } } });
	expect(result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.html).join(', ')}`)).toEqual(
		[]
	);
}

describe('InlineError', () => {
	it('renders nothing without a message and an alert with one', async () => {
		const { rerender } = render(InlineError, { props: { message: '' } });
		expect(screen.queryByRole('alert')).not.toBeInTheDocument();

		const ondismiss = vi.fn();
		await rerender({ message: 'Deploy failed', title: 'Couldn’t deploy', ondismiss });
		expect(screen.getByRole('alert')).toHaveTextContent('Couldn’t deploy');
		expect(screen.getByRole('alert')).toHaveTextContent('Deploy failed');
		await userEvent.click(screen.getByRole('button', { name: /dismiss error/i }));
		expect(ondismiss).toHaveBeenCalledOnce();
		await expectNoAxeViolations();
	});
});

describe('SuccessAlert', () => {
	it('keeps a polite live region mounted and auto-dismisses', async () => {
		vi.useFakeTimers();
		const ondismiss = vi.fn();
		const { rerender } = render(SuccessAlert, { props: { message: '', ondismiss, timeout: 3000 } });
		const region = screen.getByRole('status');
		expect(region).toBeEmptyDOMElement();

		await rerender({ message: 'Saved.', ondismiss, timeout: 3000 });
		expect(screen.getByRole('status')).toHaveTextContent('Saved.');
		vi.advanceTimersByTime(3000);
		expect(ondismiss).toHaveBeenCalledOnce();
		vi.useRealTimers();
	});
});

describe('ConfirmDialog', () => {
	it('requires the typed confirmation and shows failures inside the dialog', async () => {
		const user = userEvent.setup();
		const onconfirm = vi.fn().mockRejectedValueOnce(new Error('Database is busy'));
		render(ConfirmDialog, {
			props: {
				open: true,
				title: 'Delete main-db?',
				description: 'All data is deleted.',
				confirmLabel: 'Delete database',
				typedConfirmation: 'main-db',
				onconfirm
			}
		});

		const dialog = await screen.findByRole('alertdialog');
		const submit = screen.getByRole('button', { name: 'Delete database' });
		expect(submit).toBeDisabled();
		await user.type(screen.getByLabelText(/type main-db to confirm/i), 'main-d');
		expect(submit).toBeDisabled();
		await user.type(screen.getByLabelText(/type main-db to confirm/i), 'b');
		expect(submit).toBeEnabled();
		await expectNoAxeViolations();

		await user.click(submit);
		expect(onconfirm).toHaveBeenCalledOnce();
		expect(await screen.findByText('Database is busy')).toBeInTheDocument();
		expect(dialog).toBeInTheDocument();
	});
});

describe('confirmAction', () => {
	it('resolves true after the action succeeds and false on cancel', async () => {
		const user = userEvent.setup();
		renderConfirmHost();
		const onconfirm = vi.fn().mockResolvedValue(undefined);

		const first = confirmAction({ title: 'Stop api?', confirmLabel: 'Stop', onconfirm });
		await confirmDialog(user);
		await expect(first).resolves.toBe(true);
		expect(onconfirm).toHaveBeenCalledOnce();

		const second = confirmAction({ title: 'Stop api?', details: ['It goes offline.'] });
		expect(await screen.findByText('It goes offline.')).toBeInTheDocument();
		await cancelDialog(user);
		await expect(second).resolves.toBe(false);
	});
});
