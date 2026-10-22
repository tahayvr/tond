<!--
  AdminSetupWarning — a template app whose admin account is still up for
  grabs: the first visitor creates it (admin_setup 'open'), or Tond couldn't
  pre-seed it ('failed'). Shown on the app page until dismissed.

  <AdminSetupWarning app={current} ondismissed={(updated) => (app = updated)} />
-->
<script lang="ts">
	import ShieldIcon from '~icons/ri/shield-user-line';
	import ExternalIcon from '~icons/ri/external-link-line';
	import { Button } from '$lib/components/ui/button/index.js';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { useAction } from '$lib/action.svelte';
	import { dismissAdminSetup, type App } from '$lib/api';

	let {
		app,
		canDismiss = true,
		ondismissed
	}: {
		app: Pick<App, 'id' | 'domain' | 'admin_setup' | 'admin_setup_error'>;
		/** Dismissing changes the app (apps:write). */
		canDismiss?: boolean;
		ondismissed?: (app: App) => void;
	} = $props();

	// 'open': the first visitor creates the admin; 'failed': Tond couldn't seed it.
	let warns = $derived(app.admin_setup === 'open' || app.admin_setup === 'failed');

	let url = $derived.by(() => {
		const domain = app.domain;
		if (!domain) return null;
		const local = domain === 'localhost' || domain.endsWith('.localhost');
		return `${local ? 'http' : 'https'}://${domain}`;
	});

	const dismiss = useAction(() => dismissAdminSetup(app.id), {
		onSuccess: (updated) => ondismissed?.(updated)
	});
</script>

{#if warns}
	<section
		aria-labelledby="admin-setup-warning-title"
		data-testid="admin-setup-warning"
		class="flex w-full items-start gap-3 rounded-sm border border-warn/40 bg-warn/10 px-3 py-2.5 text-sm text-warn"
	>
		<ShieldIcon aria-hidden="true" class="mt-0.5 size-4 shrink-0" />
		<div class="min-w-0 flex-1 space-y-2">
			<h2 id="admin-setup-warning-title" class="font-semibold">
				{app.admin_setup === 'failed'
					? "Tond couldn't create the admin account"
					: 'Create the admin account now'}
			</h2>
			{#if app.admin_setup === 'failed' && app.admin_setup_error}
				<p class="break-words">{app.admin_setup_error}</p>
			{/if}
			<p>Open the app now and create the admin account; anyone who reaches it first can.</p>
			<div class="flex flex-wrap gap-2">
				{#if url}
					<Button size="sm" variant="outline" href={url} target="_blank" rel="noopener noreferrer">
						<ExternalIcon aria-hidden="true" class="size-4" /> Open the app
					</Button>
				{/if}
				{#if canDismiss}
					<Button
						size="sm"
						variant="ghost"
						onclick={() => dismiss.run()}
						disabled={dismiss.pending}
						aria-busy={dismiss.pending}
					>
						{dismiss.pending ? 'Dismissing…' : "I've created it, dismiss"}
					</Button>
				{/if}
			</div>
			<InlineError message={dismiss.error} ondismiss={dismiss.clear} />
		</div>
	</section>
{/if}
