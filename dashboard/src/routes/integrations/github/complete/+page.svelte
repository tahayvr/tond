<script lang="ts">
	// GitHub redirects the browser back to Tond after creating or installing a
	// GitHub App. That cross-site navigation doesn't carry the SameSite=Strict
	// session cookie, so the server hands off to this page, which finishes the
	// flow with a same-origin request.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import Github from '~icons/ri/github-fill';
	import { completeGitHubFlow } from '$lib/api';

	let started = false;

	$effect(() => {
		if (started) return;
		started = true;

		const params = page.url.searchParams;
		const kind = params.get('kind') === 'install' ? 'install' : 'manifest';
		const installationId = Number(params.get('installation_id'));

		completeGitHubFlow({
			kind,
			state: params.get('state'),
			code: params.get('code'),
			installation_id: Number.isFinite(installationId) && installationId > 0 ? installationId : null
		})
			.then(() => goto('/integrations/github', { replaceState: true }))
			.catch((e: any) => {
				const code = /^[a-z_]+$/.test(e?.message ?? '') ? e.message : 'internal';
				goto(`/integrations/github?error=${code}`, { replaceState: true });
			});
	});
</script>

<div class="flex min-h-[60vh] flex-col items-center justify-center gap-4 text-center">
	<Github class="size-10 animate-pulse text-muted-foreground" />
	<p class="text-sm text-muted-foreground">Finishing GitHub setup…</p>
</div>
