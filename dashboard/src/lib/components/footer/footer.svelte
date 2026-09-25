<script lang="ts">
	import SystemStats from './system-stats.svelte';
	import TerminalIcon from '~icons/ri/terminal-box-line';
	import { Button } from '$lib/components/ui/button/index.js';
	import { getTerminalOpen, setTerminalOpen } from '$lib/ui-context.svelte';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';

	let terminalOpen = $derived(getTerminalOpen());

	function handleToggle() {
		setTerminalOpen(!terminalOpen);
	}
</script>

<div class="flex h-8 w-full items-center justify-between border-t px-6">
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
	<SystemStats />
</div>
