<!-- Cmd/Ctrl+K palette: jump to any page or resource, or run a common action. -->
<script lang="ts">
	import { docsUrl } from '$lib/docs';
	import { goto } from '$app/navigation';
	import { toggleMode } from 'mode-watcher';
	import * as Command from '$lib/components/ui/command/index.js';
	import { deployApp, listApps, listCronJobs, listDatabases, listProjects } from '$lib/api';
	import { can } from '$lib/auth.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { errorMessage } from '$lib/action.svelte';
	import {
		buildPaletteEntries,
		GROUP_ORDER,
		type PaletteData,
		type PaletteEntry
	} from '$lib/command-palette';
	import { getTerminalOpen, setTerminalOpen } from '$lib/ui-context.svelte';

	let {
		open = $bindable(false),
		onShowShortcuts
	}: { open?: boolean; onShowShortcuts: () => void } = $props();

	let data = $state<PaletteData>({ apps: [], databases: [], cronJobs: [], projects: [] });
	let loading = $state(false);
	let loadError = $state('');

	// Fetch fresh resources each time the palette opens (no background polling).
	$effect(() => {
		if (!open) return;
		let cancelled = false;
		loading = true;
		loadError = '';
		Promise.allSettled([listApps(), listDatabases(), listCronJobs(), listProjects()]).then(
			([apps, databases, cronJobs, projects]) => {
				if (cancelled) return;
				data = {
					apps: apps.status === 'fulfilled' ? apps.value : [],
					databases: databases.status === 'fulfilled' ? databases.value : [],
					cronJobs: cronJobs.status === 'fulfilled' ? cronJobs.value : [],
					projects: projects.status === 'fulfilled' ? projects.value : []
				};
				const failed = [apps, databases, cronJobs, projects].find((r) => r.status === 'rejected');
				if (failed && failed.status === 'rejected') {
					loadError = `Some resources couldn't be loaded: ${errorMessage(failed.reason)}`;
				}
				loading = false;
			}
		);
		return () => {
			cancelled = true;
		};
	});

	let entries = $derived(buildPaletteEntries(data, { can }));
	let groups = $derived(
		GROUP_ORDER.map((group) => ({ group, items: entries.filter((e) => e.group === group) })).filter(
			(g) => g.items.length > 0
		)
	);

	function handleSelect(entry: PaletteEntry) {
		open = false;
		if (entry.href) {
			goto(entry.href);
			return;
		}
		const action = entry.action;
		if (!action) return;
		switch (action.kind) {
			case 'theme':
				toggleMode();
				break;
			case 'terminal':
				setTerminalOpen(!getTerminalOpen());
				break;
			case 'shortcuts':
				onShowShortcuts();
				break;
			case 'docs':
				window.open(docsUrl(), '_blank', 'noopener');
				break;
			case 'deploy': {
				let deploymentId = '';
				confirmAction({
					title: `Deploy ${action.appName}?`,
					description:
						'Tond builds the current source and swaps traffic over once the new container is healthy.',
					confirmLabel: 'Deploy',
					pendingLabel: 'Starting deploy…',
					destructive: false,
					onconfirm: async () => {
						deploymentId = (await deployApp(action.appId)).deployment_id;
					}
				}).then((ok) => {
					// Follow the deploy live, like the app page's Deploy button.
					if (ok) goto(`/apps/${action.appId}/deployments/${deploymentId}`);
				});
				break;
			}
		}
	}
</script>

<Command.Dialog
	bind:open
	title="Command palette"
	description="Jump to a page, app, database or cron job, or run an action"
>
	<Command.Input placeholder="Search apps, databases, pages, actions…" />
	<Command.List class="max-h-[60vh]">
		{#if loading}
			<Command.Loading>Loading resources…</Command.Loading>
		{/if}
		{#if loadError}
			<p role="alert" class="px-3 py-2 text-xs text-destructive">{loadError}</p>
		{/if}
		<Command.Empty>No matches. Try an app name, a page or “deploy”.</Command.Empty>
		{#each groups as { group, items } (group)}
			<Command.Group heading={group}>
				{#each items as entry (entry.id)}
					<Command.Item
						value={entry.id}
						keywords={[entry.label, ...entry.keywords]}
						onSelect={() => handleSelect(entry)}
					>
						<span class="truncate">{entry.label}</span>
						{#if entry.hint}
							<span class="ml-auto truncate font-mono text-xs text-muted-foreground"
								>{entry.hint}</span
							>
						{/if}
					</Command.Item>
				{/each}
			</Command.Group>
		{/each}
	</Command.List>
</Command.Dialog>
