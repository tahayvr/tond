<script lang="ts">
	import TondLogo from '../brand/tond-logo.svelte';
	import DesktopNav from '../nav/desktop-nav.svelte';
	import MobileNav from '../nav/mobile-nav.svelte';
	import ToggleTheme from '../nav/toggle-theme.svelte';
	import { clearAuth, getUser } from '$lib/auth.svelte';
	import { logout } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setPaletteOpen } from '$lib/ui-context.svelte';
	import SearchIcon from '~icons/ri/search-line';

	async function handleLogout() {
		try {
			await logout();
		} catch {
			// clear local state even if API call fails
		}
		clearAuth();
	}
</script>

<header
	class="flex h-12 items-center justify-between border-b-2 bg-background py-2 pr-2 pl-2 lg:pl-4"
>
	<div class="flex min-w-0 items-center gap-2 lg:gap-6">
		<div class="lg:hidden">
			<MobileNav email={getUser()?.email} onLogout={handleLogout} />
		</div>

		<a href="/" class="rounded-sm" aria-label="Tond home">
			<TondLogo class="h-5 w-auto" />
		</a>

		<div class="hidden items-center lg:flex">
			<DesktopNav />
		</div>
	</div>
	<div class="flex items-center gap-2">
		<Button
			variant="outline"
			size="sm"
			class="h-8 gap-2 px-2 text-xs font-normal text-muted-foreground normal-case sm:w-44 sm:justify-start"
			onclick={() => setPaletteOpen(true)}
			aria-label="Search and run commands"
			aria-keyshortcuts="Meta+K Control+K"
		>
			<SearchIcon class="size-4" />
			<span class="hidden sm:inline">Search…</span>
			<kbd class="ml-auto hidden rounded-sm border bg-muted px-1 font-mono text-[11px] sm:inline"
				>⌘K</kbd
			>
		</Button>
		<Button
			href="/profile"
			variant="ghost"
			size="sm"
			class="hidden max-w-48 truncate text-xs text-muted-foreground lowercase xl:inline-flex"
		>
			{getUser()?.email}
		</Button>
		<Button
			onclick={handleLogout}
			variant="ghost"
			size="sm"
			class="hidden text-xs text-muted-foreground lg:inline-flex"
		>
			Logout
		</Button>
		<ToggleTheme />
	</div>
</header>
