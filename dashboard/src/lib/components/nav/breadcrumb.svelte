<script lang="ts">
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

<nav class="min-w-0 text-sm text-muted-foreground" aria-label="Breadcrumb">
	<ol class="flex min-w-0 items-center gap-1">
		<li class="flex shrink-0 items-center">
			<a href="/" class="flex items-center rounded-sm transition-colors hover:text-foreground">
				<Home aria-hidden="true" class="h-4 w-4" />
				<span class="sr-only">Home</span>
			</a>
		</li>
		{#each items as item, i (i)}
			{@const last = i === items.length - 1}
			<li class="flex min-w-0 items-center gap-1">
				<ChevronRight aria-hidden="true" class="mx-1 h-3 w-3 shrink-0" />
				{#if last || !item.href}
					<span
						class={last ? 'truncate font-medium text-foreground' : 'truncate'}
						aria-current={last ? 'page' : undefined}
					>
						{item.label}
					</span>
				{:else}
					<a href={item.href} class="truncate transition-colors hover:text-foreground hover:underline">
						{item.label}
					</a>
				{/if}
			</li>
		{/each}
	</ol>
</nav>
