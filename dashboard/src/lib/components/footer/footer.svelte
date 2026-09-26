<script lang="ts">
	import SystemStats from './system-stats.svelte';
	import TerminalIcon from '~icons/ri/terminal-box-line';
	import { Button } from '$lib/components/ui/button/index.js';
	import { getTerminalOpen, setTerminalOpen } from '$lib/ui-context.svelte';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import { useSidebar } from '$lib/components/ui/sidebar/context.svelte.js';
	import FeedIcon from '~icons/ri/route-line';

	const sidebar = useSidebar();
	let feedOpen = $derived(sidebar ? (sidebar.isMobile ? sidebar.openMobile : sidebar.open) : false);

	let terminalOpen = $derived(getTerminalOpen());

	function handleToggle() {
		setTerminalOpen(!terminalOpen);
	}
</script>

<div class="flex h-8 w-full items-center justify-between gap-2 border-t px-2 sm:px-6">
	<div class="flex items-center gap-1">
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button
						{...props}
						variant={terminalOpen ? 'secondary' : 'ghost'}
						size="sm"
						aria-label="Toggle terminal panel"
						class={terminalOpen ? 'bg-transparent text-success' : 'bg-transparent '}
						onclick={handleToggle}
					>
						<TerminalIcon class="size-3.5" />
						<span class="hidden text-xs sm:inline">Terminal</span>
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>
				<p>{terminalOpen ? 'Close terminal' : 'Open terminal'} (Ctrl+`)</p>
			</Tooltip.Content>
		</Tooltip.Root>
		{#if sidebar && !terminalOpen}
			<Button
				variant="ghost"
				size="sm"
				class={feedOpen ? 'bg-transparent text-success' : 'bg-transparent'}
				aria-label={feedOpen ? 'Hide log feed' : 'Show log feed'}
				aria-pressed={feedOpen}
				onclick={() => sidebar.toggle()}
			>
				<FeedIcon class="size-3.5" />
				<span class="hidden text-xs sm:inline">Feed</span>
			</Button>
		{/if}
	</div>
	<SystemStats />
</div>
