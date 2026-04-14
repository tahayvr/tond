<script lang="ts">
	import type { GitHubApp } from '$lib/api';

	interface Props {
		app: GitHubApp;
		disconnecting: boolean;
		onDisconnect: () => void;
		onInstall: () => void;
		onRename: (updated: GitHubApp) => void;
	}

	let { app, disconnecting, onDisconnect, onInstall, onRename }: Props = $props();
</script>

<div>
	<p>{app.name}</p>
	<button type="button" onclick={onInstall}>Install app</button>
	<button type="button" onclick={onDisconnect} disabled={disconnecting}>Disconnect app</button>
	<button type="button" onclick={() => onRename({ ...app, name: `${app.name} renamed` })}
		>Rename app</button
	>
</div>
