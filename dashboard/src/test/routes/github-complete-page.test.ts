import { render } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { goto, completeGitHubFlow } = vi.hoisted(() => ({
	goto: vi.fn(),
	completeGitHubFlow: vi.fn()
}));

vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$app/state', () => ({
	page: {
		url: new URL('http://localhost/integrations/github/complete?kind=manifest&code=c0de&state=s7')
	}
}));
vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, completeGitHubFlow };
});

import { ApiError } from '$lib/api';
import CompletePage from '../../routes/integrations/github/complete/+page.svelte';

describe('integrations/github/complete', () => {
	beforeEach(() => {
		goto.mockReset();
		completeGitHubFlow.mockReset();
	});

	it('finishes the flow and returns to the GitHub integration', async () => {
		completeGitHubFlow.mockResolvedValue(undefined);

		render(CompletePage);

		await vi.waitFor(() =>
			expect(goto).toHaveBeenCalledWith('/integrations/github', { replaceState: true })
		);
		expect(completeGitHubFlow).toHaveBeenCalledWith(
			expect.objectContaining({ kind: 'manifest', code: 'c0de', state: 's7' })
		);
	});

	it('leaves a 401 to the login redirect so the flow resumes after sign-in', async () => {
		completeGitHubFlow.mockRejectedValue(new ApiError('Your session has expired.', 401));

		render(CompletePage);

		await vi.waitFor(() => expect(completeGitHubFlow).toHaveBeenCalled());
		await Promise.resolve();
		expect(goto).not.toHaveBeenCalled();
	});
});
