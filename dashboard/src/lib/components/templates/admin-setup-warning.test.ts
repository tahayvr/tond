import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { App, Template } from '$lib/api';
import templatesJson from '../../../mocks/templates.json';

const { dismissAdminSetup } = vi.hoisted(() => ({ dismissAdminSetup: vi.fn() }));
vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	dismissAdminSetup
}));

import { ApiError } from '$lib/api';
import AdminSetupWarning from './admin-setup-warning.svelte';

type Shown = Pick<App, 'id' | 'domain' | 'admin_setup' | 'admin_setup_error'>;
const app = (over: Partial<Shown> = {}): Shown => ({
	id: 'app_1',
	domain: 'notes.example.com',
	admin_setup: 'open',
	admin_setup_error: null,
	...over
});

describe('AdminSetupWarning', () => {
	beforeEach(() => {
		dismissAdminSetup.mockReset();
	});

	it('tells the user to claim the admin account now, with a link to the app', () => {
		render(AdminSetupWarning, { props: { app: app() } });
		expect(
			screen.getByRole('heading', { name: 'Create the admin account now' })
		).toBeInTheDocument();
		expect(
			screen.getByText(
				'Open the app now and create the admin account; anyone who reaches it first can.'
			)
		).toBeInTheDocument();
		expect(screen.getByRole('link', { name: /Open the app/ })).toHaveAttribute(
			'href',
			'https://notes.example.com'
		);
	});

	it('explains a failed pre-seed with its error', () => {
		render(AdminSetupWarning, {
			props: { app: app({ admin_setup: 'failed', admin_setup_error: 'gave up after 120 seconds' }) }
		});
		expect(
			screen.getByRole('heading', { name: "Tond couldn't create the admin account" })
		).toBeInTheDocument();
		expect(screen.getByText('gave up after 120 seconds')).toBeInTheDocument();
	});

	it('renders nothing once the admin exists or the warning was dismissed', () => {
		for (const admin_setup of ['pending', 'done', 'dismissed', null] as const) {
			const { unmount } = render(AdminSetupWarning, { props: { app: app({ admin_setup }) } });
			expect(screen.queryByTestId('admin-setup-warning')).not.toBeInTheDocument();
			unmount();
		}
	});

	it('dismisses through the API and hands back the updated app', async () => {
		const user = userEvent.setup();
		const updated = { ...app(), admin_setup: 'dismissed' };
		dismissAdminSetup.mockResolvedValue(updated);
		const ondismissed = vi.fn();
		render(AdminSetupWarning, { props: { app: app(), ondismissed } });

		await user.click(screen.getByRole('button', { name: "I've created it, dismiss" }));
		expect(dismissAdminSetup).toHaveBeenCalledWith('app_1');
		await waitFor(() => expect(ondismissed).toHaveBeenCalledWith(updated));
	});

	it('shows a failed dismiss next to the warning', async () => {
		const user = userEvent.setup();
		dismissAdminSetup.mockRejectedValue(new ApiError('Not allowed', 403));
		render(AdminSetupWarning, { props: { app: app() } });

		await user.click(screen.getByRole('button', { name: "I've created it, dismiss" }));
		expect(await screen.findByRole('alert')).toHaveTextContent('Not allowed');
	});

	it('hides the dismiss button from users who cannot change the app', () => {
		render(AdminSetupWarning, { props: { app: app(), canDismiss: false } });
		expect(screen.queryByRole('button', { name: /dismiss/ })).not.toBeInTheDocument();
	});

	it('matches the catalog: seeded templates never warn, first-visitor ones do', () => {
		const catalog = templatesJson as Template[];
		const setup = (id: string) => catalog.find((t) => t.id === id)?.admin_setup;
		expect(setup('gitea')).toBe('seeded');
		expect(setup('umami')).toBe('seeded');
		expect(setup('memos')).toBe('first_visitor');
		expect(setup('ghost')).toBe('first_visitor');
	});
});
