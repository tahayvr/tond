<script lang="ts">
	import { goto } from '$app/navigation';
	import TondLogo from '../brand/tond-logo.svelte';
	import DesktopNav from '../nav/desktop-nav.svelte';
	import MobileNav from '../nav/mobile-nav.svelte';
	import ToggleTheme from '../nav/toggle-theme.svelte';
	import { clearAuth, getUser } from '$lib/auth.svelte';
	import { logout } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';

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

		<button onclick={() => goto('/')} class="hover:cursor-pointer" aria-label="Tond home">
			<TondLogo class="h-5 w-auto" />
		</button>

		<div class="hidden items-center lg:flex">
			<DesktopNav />
		</div>
	</div>
	<div class="flex items-center gap-2">
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
