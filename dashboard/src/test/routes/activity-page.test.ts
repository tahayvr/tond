import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AuditLogEntry } from '$lib/api';

const { setUiContext, clearUiContext, projectsContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));
const { listAuditLog, verifyAuditLog } = vi.hoisted(() => ({
	listAuditLog: vi.fn(),
	verifyAuditLog: vi.fn()
}));
const { goto } = vi.hoisted(() => ({ goto: vi.fn() }));

vi.mock('$lib/ui-context.svelte', () => ({ setUiContext, clearUiContext, projectsContext }));
vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, listAuditLog, verifyAuditLog };
});

import ActivityPage from '../../routes/activity/+page.svelte';

const entry = (seq: number, extra: Partial<AuditLogEntry> = {}): AuditLogEntry => ({
	seq,
	created_at: new Date().toISOString(),
	actor_kind: 'token',
	actor_id: 'tok_1',
	actor_label: 'ci',
	action: 'deploy',
	resource_type: 'app',
	resource_id: 'app_1',
	resource_name: 'api-service',
	outcome: 'success',
	ip: '203.0.113.4',
	detail: null,
	...extra
});

const filters = { q: '', actor: '', action: '', resourceType: '', failuresOnly: false };

function renderPage(entries: AuditLogEntry[], nextBefore: number | null = null) {
	render(ActivityPage, { props: { params: {}, data: { filters, entries, nextBefore } } });
}

describe('activity page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		listAuditLog.mockReset();
		verifyAuditLog.mockReset();
		goto.mockReset();
	});

	it('shows who did what, with tokens named as tokens and failures flagged', () => {
		renderPage([
			entry(2),
			entry(1, {
				actor_kind: 'anonymous',
				actor_label: 'root@example.com',
				action: 'login_failed',
				outcome: 'failure',
				resource_type: null,
				resource_name: null
			})
		]);
		expect(screen.getByText('token ci')).toBeInTheDocument();
		expect(screen.getByText('deployed')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'api-service' })).toHaveAttribute(
			'href',
			'/apps/app_1'
		);
		expect(screen.getByText('failed to sign in')).toBeInTheDocument();
		expect(screen.getByText('failed')).toBeInTheDocument();
	});

	it('loads older pages from the server with the cursor', async () => {
		const user = userEvent.setup();
		listAuditLog.mockResolvedValue({
			entries: [entry(1, { actor_label: 'older' })],
			next_before: null
		});
		renderPage([entry(5)], 5);

		await user.click(screen.getByRole('button', { name: /load older events/i }));

		expect(listAuditLog).toHaveBeenCalledWith(expect.objectContaining({ before: 5, limit: 50 }));
		expect(await screen.findByText('token older')).toBeInTheDocument();
		expect(screen.getByText(/that's everything/i)).toBeInTheDocument();
	});

	it('filters by actor through the URL when a name is clicked', async () => {
		const user = userEvent.setup();
		goto.mockResolvedValue(undefined);
		renderPage([entry(2)]);

		await user.click(screen.getByRole('button', { name: /token ci/i }));

		expect(goto).toHaveBeenCalledWith('/activity?actor=ci', expect.anything());
	});

	it('reports the integrity check', async () => {
		const user = userEvent.setup();
		verifyAuditLog.mockResolvedValue({
			ok: true,
			checked: 12,
			checkpoint_seq: 0,
			first_seq: 1,
			last_seq: 12,
			broken_at: null,
			problem: null
		});
		renderPage([entry(2)]);

		await user.click(screen.getByRole('button', { name: /verify integrity/i }));

		expect(await screen.findByText(/12 entries checked/i)).toBeInTheDocument();
	});

	it('explains an empty result', () => {
		renderPage([]);
		expect(screen.getByText('No matching events')).toBeInTheDocument();
	});
});
