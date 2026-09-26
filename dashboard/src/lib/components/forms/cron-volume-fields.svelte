<script lang="ts" module>
	import type { CronVolumeInput } from '$lib/api';

	/** Sentinel select value for "this job's own folder". */
	export const OWN_FOLDER = '__own__';

	/** Mirrors the server's default volume name: "/data/cache" → "data_cache". */
	export function defaultVolumeName(containerPath: string): string {
		return (
			containerPath
				.replace(/^\/+|\/+$/g, '')
				.replace(/[^A-Za-z0-9._-]/g, '_')
				.replace(/^\.+/, '') || 'data'
		);
	}

	/** Short label for a volume's host side, e.g. "web / uploads" or "job / scratch". */
	export function cronVolumeLabel(v: CronVolumeInput): string {
		const name = v.name || defaultVolumeName(v.container_path);
		return v.app ? `app ${v.app} / ${name}` : `job / ${name}`;
	}

	/** Client-side checks that mirror the API's rules, for instant feedback. */
	export function validateCronVolume(name: string, containerPath: string): string {
		if (!containerPath.startsWith('/')) {
			return 'Container path must be absolute, e.g. /data';
		}
		if (name && !/^[A-Za-z0-9_-][A-Za-z0-9._-]{0,63}$/.test(name)) {
			return "Name may only use letters, digits, '.', '_' and '-' (no slashes, not starting with '.')";
		}
		return '';
	}
</script>

<script lang="ts">
	import { listApps, type App } from '$lib/api';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';

	interface Props {
		/** OWN_FOLDER or an app name */
		source: string;
		name: string;
		containerPath: string;
		onkeydown?: (e: KeyboardEvent) => void;
	}

	let {
		source = $bindable(OWN_FOLDER),
		name = $bindable(''),
		containerPath = $bindable(''),
		onkeydown
	}: Props = $props();

	let apps = $state<App[]>([]);
	$effect(() => {
		listApps()
			.then((list) => (apps = list))
			.catch(() => (apps = []));
	});

	let sourceLabel = $derived(source === OWN_FOLDER ? "This job's own folder" : `App: ${source}`);
	let effectiveName = $derived(name.trim() || defaultVolumeName(containerPath.trim()));
</script>

<div class="space-y-4 py-2">
	<div class="space-y-2">
		<Label for="vol-source">Stored in</Label>
		<Select.Root type="single" bind:value={source}>
			<Select.Trigger id="vol-source" class="w-full">{sourceLabel}</Select.Trigger>
			<Select.Content>
				<Select.Item value={OWN_FOLDER} label="This job's own folder" />
				{#each apps as app (app.id)}
					<Select.Item value={app.name} label={`App: ${app.name}`} />
				{/each}
			</Select.Content>
		</Select.Root>
		<p class="text-xs text-muted-foreground">
			Pick an app to share one of its volumes (e.g. to back up its data).
		</p>
	</div>
	<div class="space-y-2">
		<Label for="vol-name">Volume name <span class="text-muted-foreground">(optional)</span></Label>
		<Input
			id="vol-name"
			bind:value={name}
			placeholder={defaultVolumeName(containerPath.trim())}
			class="font-mono"
			{onkeydown}
		/>
	</div>
	<div class="space-y-2">
		<Label for="vol-container">Container path</Label>
		<Input
			id="vol-container"
			bind:value={containerPath}
			placeholder="/data"
			class="font-mono"
			{onkeydown}
		/>
	</div>
	<p class="text-xs text-muted-foreground">
		Host folder:
		<span class="font-mono"
			>/opt/tond/data/volumes/{source === OWN_FOLDER
				? '_cron/<job-id>'
				: source}/{effectiveName}</span
		>. Other host paths can't be mounted.
	</p>
</div>
