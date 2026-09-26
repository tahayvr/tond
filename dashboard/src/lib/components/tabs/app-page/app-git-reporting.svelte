<!--
  Commit statuses and pull-request previews for a git app: which forge
  credential Tond reports through. A GitHub App link is shown as is (it's
  set when the app is created); GitLab and Gitea/Forgejo tokens are picked
  here.
-->
<script lang="ts">
	import {
		listGitLabTokens,
		listGiteaTokens,
		setAppGitLabToken,
		setAppGiteaToken,
		type App,
		type GitLabToken,
		type GiteaToken
	} from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import CheckIcon from '~icons/ri/checkbox-circle-line';
	import Github from '~icons/ri/github-fill';

	interface Props {
		app: App;
		onchange?: (app: App) => void;
	}

	let { app, onchange }: Props = $props();

	let gitlab = $state<GitLabToken[]>([]);
	let gitea = $state<GiteaToken[]>([]);
	let loadError = $state('');

	$effect(() => {
		Promise.all([listGitLabTokens(), listGiteaTokens()])
			.then(([gl, gt]) => {
				gitlab = gl;
				gitea = gt;
			})
			.catch((e) => (loadError = e.message ?? 'Could not load the tokens'));
	});

	/** "gitlab:<id>" | "gitea:<id>" | "" */
	let current = $derived(
		app.gitlab_token_id
			? `gitlab:${app.gitlab_token_id}`
			: app.gitea_token_id
				? `gitea:${app.gitea_token_id}`
				: ''
	);
	let selected = $state('');
	$effect(() => {
		selected = current;
	});

	function label(value: string): string {
		if (!value) return 'Nothing (no statuses or previews)';
		const [kind, id] = value.split(':');
		const t =
			kind === 'gitlab' ? gitlab.find((x) => x.id === id) : gitea.find((x) => x.id === id);
		return t ? `${t.name} (${kind === 'gitlab' ? 'GitLab' : 'Gitea / Forgejo'})` : 'A removed token';
	}

	const save = useAction(
		async (value: string) => {
			const [kind, id] = value ? value.split(':') : ['', ''];
			// One forge at a time: link the chosen token, unlink the other kind.
			let updated = app;
			if (kind !== 'gitlab' && app.gitlab_token_id) updated = await setAppGitLabToken(app.id, null);
			if (kind !== 'gitea' && app.gitea_token_id) updated = await setAppGiteaToken(app.id, null);
			if (kind === 'gitlab') updated = await setAppGitLabToken(app.id, id);
			if (kind === 'gitea') updated = await setAppGiteaToken(app.id, id);
			return updated;
		},
		{
			success: (_r, value) =>
				value
					? `Deploys and previews of ${app.name} are now reported through ${label(value)}.`
					: `${app.name} no longer reports to a forge.`,
			invalidate: true,
			onSuccess: (updated) => onchange?.(updated)
		}
	);

	function handleChange(value: string) {
		if (value === current) return;
		void save.run(value);
	}
</script>

<section class="grid gap-3 rounded-md border p-4" data-testid="app-git-reporting">
	<div>
		<h3 class="text-sm font-medium">Commit statuses and previews</h3>
		<p class="mt-1 text-sm text-muted-foreground">
			Tond marks each deployed commit with a <span class="font-mono text-xs">tond/deploy</span>
			status linking to the deployment, and — with previews on — deploys every pull or merge request with
			a comment linking to its preview.
		</p>
	</div>

	{#if app.github_app_id}
		<p class="flex items-center gap-2 text-sm">
			<Github class="size-4" />
			<CheckIcon class="size-4 text-success" />
			Reported through the linked GitHub App.
		</p>
	{:else}
		<div class="grid max-w-md gap-2">
			<Label for="git-reporting-token">Report through</Label>
			<Select.Root
				type="single"
				bind:value={selected}
				onValueChange={handleChange}
				disabled={save.pending}
			>
				<Select.Trigger id="git-reporting-token" aria-label="Forge token">
					{save.pending ? 'Saving…' : label(selected)}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">Nothing (no statuses or previews)</Select.Item>
					{#each gitlab as t (t.id)}
						<Select.Item value={`gitlab:${t.id}`}>{t.name} (GitLab)</Select.Item>
					{/each}
					{#each gitea as t (t.id)}
						<Select.Item value={`gitea:${t.id}`}>{t.name} (Gitea / Forgejo)</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			<p class="text-xs text-muted-foreground">
				Add GitLab or Gitea/Forgejo tokens under
				<a class="underline" href="/integrations">Integrations</a>. The token also clones the
				repository if it's private.
			</p>
		</div>
	{/if}

	<InlineError message={loadError} />
	<InlineError message={save.error} ondismiss={save.clear} />
	<SuccessAlert message={save.success} ondismiss={save.clear} timeout={6000} />
</section>
