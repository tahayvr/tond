<script lang="ts">
	import { goto } from '$app/navigation';
	import ChevronRight from '~icons/ri/arrow-right-s-line';
	import Home from '~icons/ri/home-4-line';

	interface BreadcrumbItem {
		label: string;
		href?: string;
	}

	interface Props {
		items: BreadcrumbItem[];
	}

	let { items }: Props = $props();
</script>

<nav class="flex items-center gap-1 text-sm text-muted-foreground" aria-label="Breadcrumb">
	<button
		onclick={() => goto('/projects')}
		class="flex items-center transition-colors hover:text-foreground"
		title="Home"
	>
		<Home class="h-4 w-4" />
	</button>

	{#each items as item, i (i)}
		<ChevronRight class="mx-1 h-3 w-3" />
		{#if i === items.length - 1 || !item.href}
			<span class={i === items.length - 1 ? 'font-medium text-foreground' : ''}>
				{item.label}
			</span>
		{:else}
			<button
				onclick={() => item.href && goto(item.href)}
				class="transition-colors hover:text-foreground hover:underline"
			>
				{item.label}
			</button>
		{/if}
	{/each}
</nav>
