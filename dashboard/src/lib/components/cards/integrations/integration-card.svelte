<script lang="ts">
	import type { Component, Snippet } from 'svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import * as Card from '$lib/components/ui/card/index.js';
	import Button from '$lib/components/ui/button/button.svelte';
	import Unlink from '~icons/ri/link-unlink';
	import ChevronRight from '~icons/ri/arrow-right-s-line';

	interface Props {
		name: string;
		icon: Component<{ class?: string }>;
		status: 'connected' | 'disconnected' | 'loading';
		details?: { label: string; value: string }[];
		connectLabel?: string;
		connecting?: boolean;
		disconnecting?: boolean;
		/** When set, the card footer button navigates to this href instead of firing onconnect */
		href?: string;
		onconnect?: () => void;
		ondisconnect?: () => void;
		children?: Snippet;
	}

	let {
		name,
		icon: Icon,
		status,
		details = [],
		connectLabel = 'Connect',
		connecting = false,
		disconnecting = false,
		href,
		onconnect,
		ondisconnect,
		children
	}: Props = $props();
</script>

<Card.Root class="flex h-full w-full flex-col rounded-sm border-none p-6 transition-colors">
	<Card.Header class="p-0">
		<div class="flex items-start justify-between">
			<div class="border-2 p-3">
				<Icon class="h-8 w-8" />
			</div>
			{#if status === 'connected'}
				<StatusBadge status="connected" size="sm" />
			{:else if status === 'loading'}
				<StatusBadge status="pending" label="Connecting…" size="sm" />
			{:else}
				<StatusBadge status="not_connected" label="Not connected" size="sm" />
			{/if}
		</div>
		<Card.Title class="text-lg font-bold uppercase">{name}</Card.Title>
	</Card.Header>

	<Card.Content class="grow p-0">
		<div class="flex h-full flex-col justify-center gap-1 py-6">
			{#if details.length > 0}
				{#each details as d (d.label)}
					<div class="flex items-center justify-between border bg-background p-3 text-xs">
						<span class="text-muted-foreground uppercase">{d.label}</span>
						<span class="font-mono">{d.value}</span>
					</div>
				{/each}
			{:else if status === 'disconnected'}
				<p class="text-xs text-muted-foreground">Not connected. Click below to set up.</p>
			{/if}
			{#if children}
				{@render children()}
			{/if}
		</div>
	</Card.Content>

	<Card.Footer class="w-full flex-col gap-3 p-0">
		{#if href}
			<a {href} class="w-full">
				<Button variant="outline" class="w-full uppercase">
					<ChevronRight class="h-4 w-4" />
					Configure
				</Button>
			</a>
		{:else if status === 'connected'}
			<Button
				variant="outline"
				class="w-full uppercase"
				disabled={disconnecting}
				onclick={ondisconnect}
			>
				<Unlink class="h-4 w-4" />
				{disconnecting ? 'Disconnecting…' : 'Disconnect'}
			</Button>
		{:else}
			<Button
				class="w-full uppercase"
				disabled={connecting || status === 'loading'}
				onclick={onconnect}
			>
				{connecting || status === 'loading' ? 'Connecting…' : connectLabel}
			</Button>
		{/if}
	</Card.Footer>
</Card.Root>
