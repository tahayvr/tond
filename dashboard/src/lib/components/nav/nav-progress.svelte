<!--
  Slim loading bar at the top of the window while SvelteKit loads the next
  page. Shown only if navigation takes longer than 150 ms, so fast
  navigations don't flicker.
-->
<script lang="ts">
	import { navigating } from '$app/state';

	let visible = $state(false);
	let active = $derived(navigating.to !== null && navigating.to !== undefined);

	$effect(() => {
		if (!active) {
			visible = false;
			return;
		}
		const timer = setTimeout(() => (visible = true), 150);
		return () => clearTimeout(timer);
	});
</script>

{#if visible}
	<div
		class="pointer-events-none fixed inset-x-0 top-0 z-[100] h-0.5 overflow-hidden bg-ring/20"
		role="progressbar"
		aria-label="Loading page"
		data-testid="nav-progress"
	>
		<div class="nav-progress-bar h-full w-1/3 bg-ring motion-reduce:w-full"></div>
	</div>
{/if}

<style>
	/* A keyframe sweep can't be expressed with Tailwind utilities alone. */
	.nav-progress-bar {
		animation: nav-progress 1.1s ease-in-out infinite;
	}
	@keyframes nav-progress {
		from {
			transform: translateX(-100%);
		}
		to {
			transform: translateX(300%);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.nav-progress-bar {
			animation: none;
		}
	}
</style>
