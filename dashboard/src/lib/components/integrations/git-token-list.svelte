<!--
  Access tokens for GitLab or Gitea/Forgejo: list, add (checked against the
  instance by the server), remove. Linked to an app on its Webhooks tab, a
  token lets Tond clone private repositories, set commit statuses and comment
  preview URLs on pull/merge requests.
-->
<script lang="ts">
	import {
		createGitLabToken,
		createGiteaToken,
		deleteGitLabToken,
		deleteGiteaToken,
		type GitLabToken,
		type GiteaToken
	} from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import { can } from '$lib/auth.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import AddIcon from '~icons/ri/add-line';
	import DeleteIcon from '~icons/ri/delete-bin-line';
	import KeyIcon from '~icons/ri/key-2-line';

	type Kind = 'gitlab' | 'gitea';
	interface TokenRow {
		id: string;
		name: string;
		url: string;
		username: string;
		created_at: string;
	}

	interface Props {
		kind: Kind;
		initialGitLab?: GitLabToken[];
		initialGitea?: GiteaToken[];
	}

	let { kind, initialGitLab = [], initialGitea = [] }: Props = $props();

	const COPY: Record<
		Kind,
		{ product: string; urlLabel: string; urlDefault: string; placeholder: string; scopes: string }
	> = {
		gitlab: {
			product: 'GitLab',
			urlLabel: 'GitLab URL',
			urlDefault: 'https://gitlab.com',
			placeholder: 'https://gitlab.com',
			scopes:
				'Create a personal (or project) access token with the api scope under Preferences → Access tokens.'
		},
		gitea: {
			product: 'Gitea / Forgejo',
			urlLabel: 'Instance URL',
			urlDefault: '',
			placeholder: 'https://codeberg.org',
			scopes:
				'Create a token under Settings → Applications with read and write access to repositories and issues.'
		}
	};
	let copy = $derived(COPY[kind]);

	function rows(): TokenRow[] {
		return kind === 'gitlab'
			? initialGitLab.map((t) => ({ ...t, url: t.gitlab_url }))
			: initialGitea.map((t) => ({ ...t, url: t.base_url }));
	}

	let tokens = $state<TokenRow[]>([]);
	$effect(() => {
		tokens = rows();
	});

	let adding = $state(false);
	let name = $state('');
	let url = $state('');
	let secret = $state('');

	function openForm() {
		name = '';
		url = copy.urlDefault;
		secret = '';
		add.clear();
		adding = true;
	}

	const add = useAction(
		async () => {
			const created =
				kind === 'gitlab'
					? await createGitLabToken({ name: name.trim(), token: secret.trim(), gitlab_url: url.trim() })
					: await createGiteaToken({ name: name.trim(), token: secret.trim(), base_url: url.trim() });
			return created;
		},
		{
			success: (t) => `Token “${t.name}” saved. Link it to an app on the app's Webhooks tab.`,
			onSuccess: (t) => {
				const row: TokenRow =
					'gitlab_url' in t ? { ...t, url: t.gitlab_url } : { ...(t as GiteaToken), url: t.base_url };
				tokens = [...tokens, row];
				adding = false;
				secret = '';
			}
		}
	);

	let formValid = $derived(!!name.trim() && !!secret.trim() && /^https?:\/\/\S+/.test(url.trim()));

	function handleDelete(t: TokenRow) {
		return confirmAction({
			title: `Remove the token “${t.name}”?`,
			description:
				'Apps using it can no longer clone private repositories or report commit statuses and previews until you link another token.',
			confirmLabel: 'Remove token',
			pendingLabel: 'Removing…',
			onconfirm: async () => {
				await (kind === 'gitlab' ? deleteGitLabToken(t.id) : deleteGiteaToken(t.id));
				tokens = tokens.filter((x) => x.id !== t.id);
			}
		});
	}

	function when(iso: string): string {
		return new Date(iso).toLocaleDateString(undefined, { dateStyle: 'medium' });
	}
</script>

<div class="space-y-6" data-testid="git-token-list">
	<p class="max-w-prose text-sm text-muted-foreground">
		A token linked to an app lets Tond clone its private repository, report every deploy as a
		<span class="font-mono">tond/deploy</span> commit status, and deploy a preview for each
		{kind === 'gitlab' ? 'merge' : 'pull'} request with a comment linking to it. Tokens are encrypted
		on this server and never shown again.
	</p>

	{#if tokens.length === 0}
		<div class="rounded border border-dashed border-border px-4 py-8 text-center">
			<p class="text-sm text-muted-foreground">No {copy.product} tokens yet.</p>
		</div>
	{:else}
		<ul class="divide-y divide-border overflow-hidden rounded border border-border">
			{#each tokens as t (t.id)}
				<li class="flex flex-col gap-2 px-3 py-3 sm:flex-row sm:items-center sm:justify-between">
					<div class="min-w-0">
						<p class="flex items-center gap-2 text-sm font-medium">
							<KeyIcon class="size-4 shrink-0 text-muted-foreground" />
							{t.name}
						</p>
						<p class="truncate text-xs text-muted-foreground">
							{t.url}{t.username ? ` · ${t.username}` : ''} · added {when(t.created_at)}
						</p>
					</div>
					{#if can('settings:write')}
						<Button
							size="sm"
							variant="outline"
							class="shrink-0 self-start sm:self-auto"
							onclick={() => handleDelete(t)}
						>
							<DeleteIcon class="size-4" />
							Remove
						</Button>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}

	<SuccessAlert message={add.success} ondismiss={add.clear} timeout={8000} />

	{#if can('settings:write')}
		{#if adding}
			<form
				class="max-w-lg space-y-4 rounded-lg border p-4"
				onsubmit={(e) => {
					e.preventDefault();
					if (formValid) add.run();
				}}
			>
				<h3 class="text-sm font-semibold">Add a {copy.product} token</h3>
				<div class="grid gap-2">
					<Label for="git-token-name">Name</Label>
					<Input
						id="git-token-name"
						bind:value={name}
						placeholder="e.g. Company {copy.product.split(' ')[0]}"
						disabled={add.pending}
					/>
				</div>
				<div class="grid gap-2">
					<Label for="git-token-url">{copy.urlLabel}</Label>
					<Input
						id="git-token-url"
						bind:value={url}
						placeholder={copy.placeholder}
						inputmode="url"
						autocomplete="off"
						spellcheck={false}
						disabled={add.pending}
					/>
				</div>
				<div class="grid gap-2">
					<Label for="git-token-secret">Access token</Label>
					<Input
						id="git-token-secret"
						type="password"
						bind:value={secret}
						autocomplete="off"
						spellcheck={false}
						class="font-mono"
						disabled={add.pending}
					/>
					<p class="text-xs text-muted-foreground">{copy.scopes}</p>
				</div>
				<InlineError message={add.error} ondismiss={add.clear} />
				<div class="flex flex-wrap gap-2">
					<Button type="submit" size="sm" disabled={!formValid || add.pending}>
						{add.pending ? 'Checking the token…' : 'Save token'}
					</Button>
					<Button
						type="button"
						size="sm"
						variant="outline"
						disabled={add.pending}
						onclick={() => (adding = false)}
					>
						Cancel
					</Button>
				</div>
			</form>
		{:else}
			<Button size="sm" variant="outline" onclick={openForm}>
				<AddIcon class="size-4" />
				Add a token
			</Button>
		{/if}
	{/if}
</div>
