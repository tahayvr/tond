import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import SavedSecretField, { secretFieldUpdate } from './saved-secret-field.svelte';
import RevealableSecret from './revealable-secret.svelte';

describe('secretFieldUpdate', () => {
	it('omits a saved secret unless it is replaced or removed', () => {
		expect(secretFieldUpdate({ saved: true, replacing: false, removed: false, value: '' })).toBe(
			undefined
		);
		expect(
			secretFieldUpdate({ saved: true, replacing: true, removed: false, value: ' new-token ' })
		).toBe('new-token');
		expect(secretFieldUpdate({ saved: true, replacing: true, removed: false, value: '' })).toBe(
			undefined
		);
		expect(secretFieldUpdate({ saved: true, replacing: false, removed: true, value: '' })).toBe('');
	});

	it('sends a new value when nothing is saved yet', () => {
		expect(secretFieldUpdate({ saved: false, replacing: false, removed: false, value: 'x' })).toBe(
			'x'
		);
		expect(secretFieldUpdate({ saved: false, replacing: false, removed: false, value: ' ' })).toBe(
			undefined
		);
	});
});

describe('SavedSecretField', () => {
	it('shows a saved secret as "Saved" and never prefills it', async () => {
		const user = userEvent.setup();
		render(SavedSecretField, {
			props: { id: 'tok', saved: true, savedHint: 'hooks.slack.com', kind: 'url' }
		});

		expect(screen.getByTestId('tok-saved')).toHaveTextContent(/saved/i);
		expect(screen.getByText('hooks.slack.com')).toBeInTheDocument();
		expect(screen.queryByRole('textbox')).not.toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /replace/i }));
		const input = screen.getByRole('textbox') as HTMLInputElement;
		expect(input.value).toBe('');
		await user.click(screen.getByRole('button', { name: /keep saved/i }));
		expect(screen.getByTestId('tok-saved')).toBeInTheDocument();
	});

	it('offers an input straight away when nothing is saved', () => {
		render(SavedSecretField, { props: { id: 'tok', saved: false, kind: 'url' } });
		expect(screen.getByRole('textbox')).toBeInTheDocument();
		expect(screen.queryByTestId('tok-saved')).not.toBeInTheDocument();
	});
});

describe('RevealableSecret', () => {
	it('stays masked until an admin reveals it', async () => {
		const reveal = vi.fn().mockResolvedValue('s3cret-value');
		const user = userEvent.setup();
		render(RevealableSecret, { props: { id: 'sec', reveal, canReveal: true } });

		expect(screen.queryByDisplayValue('s3cret-value')).not.toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /reveal/i }));
		expect(reveal).toHaveBeenCalledOnce();
		expect(await screen.findByDisplayValue('s3cret-value')).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /hide secret/i }));
		expect(screen.queryByDisplayValue('s3cret-value')).not.toBeInTheDocument();
	});

	it('explains why members cannot reveal', () => {
		render(RevealableSecret, {
			props: { id: 'sec', reveal: vi.fn(), canReveal: false, lockedHint: 'Only admins.' }
		});
		expect(screen.queryByRole('button', { name: /reveal/i })).not.toBeInTheDocument();
		expect(screen.getByText('Only admins.')).toBeInTheDocument();
	});

	it('shows the server error when the reveal fails', async () => {
		const reveal = vi.fn().mockRejectedValue(new Error('Forbidden'));
		const user = userEvent.setup();
		render(RevealableSecret, { props: { id: 'sec', reveal, canReveal: true } });
		await user.click(screen.getByRole('button', { name: /reveal/i }));
		expect(await screen.findByText('Forbidden')).toBeInTheDocument();
	});
});
