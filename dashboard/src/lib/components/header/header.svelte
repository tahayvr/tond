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

<header class="flex h-12 items-center justify-between border-b-2 bg-background py-2 pr-2 pl-4">
	<div class="flex items-center gap-6">
		<button onclick={() => goto('/')} class="hover:cursor-pointer">
			<TondLogo class="h-5 w-auto" />
		</button>

		<div class="hidden items-center md:flex">
			<DesktopNav />
		</div>

		<div class="md:hidden">
			<MobileNav />
		</div>
	</div>
	<div class="flex items-center gap-2">
		<Button
			href="/profile"
			variant="ghost"
			size="sm"
			class="text-xs text-muted-foreground lowercase"
		>
			{getUser()?.email}
		</Button>
		<Button onclick={handleLogout} variant="ghost" size="sm" class="text-xs text-muted-foreground">
			Logout
		</Button>
		<ToggleTheme />
	</div>
</header>
