<script lang="ts">
	import { login } from '$lib/api';
	import { getSessionExpired, setAuthenticated } from '$lib/auth.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import TondLogo from '$lib/components/brand/tond-logo.svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';

	let email = $state('');
	let password = $state('');
	let error = $state('');
	let loading = $state(false);
	let sessionExpired = $derived(getSessionExpired());

	async function handleLogin(e: Event) {
		e.preventDefault();
		error = '';
		loading = true;

		try {
			const result = await login(email, password);
			if ('mfa_required' in result && result.mfa_required) {
				// Server issued a tond_mfa_pending cookie — continue on the 2FA page,
				// keeping ?next= so the user still lands where they were headed.
				await goto(`/login/2fa${page.url.search}`);
			} else if ('user' in result) {
				// The root layout redirects to the validated ?next= target.
				setAuthenticated(result.user);
			}
		} catch (err: any) {
			error = err.message || 'Login failed';
		} finally {
			loading = false;
		}
	}
</script>

<div class="flex min-h-[80vh] flex-col items-center justify-center">
	<div class="w-full max-w-lg space-y-8 font-mono">
		<div class="flex flex-col items-center gap-4">
			<TondLogo class="h-8 opacity-80" />
			<p class="text-xs text-muted-foreground">operator login</p>
		</div>

		{#if error}
			<p class="text-sm text-destructive" role="alert">! {error}</p>
		{:else if sessionExpired}
			<p class="text-sm text-muted-foreground" role="status">
				your session expired. sign in again to continue where you left off.
			</p>
		{/if}

		<form onsubmit={handleLogin} class="space-y-6">
			<div class="space-y-1">
				<label for="email" class="block text-xs text-muted-foreground">email</label>
				<div class="flex items-center gap-2">
					<span class="text-sm text-muted-foreground select-none">&gt;</span>
					<Input
						id="email"
						type="email"
						bind:value={email}
						placeholder="admin@example.com"
						required
						autocomplete="email"
						class="border-none bg-transparent p-1 font-mono text-sm shadow-none placeholder:text-muted-foreground/40 focus-visible:ring-0"
					/>
				</div>
				<div class="h-px bg-border"></div>
			</div>

			<div class="space-y-1">
				<label for="password" class="block text-xs text-muted-foreground">password</label>
				<div class="flex items-center gap-2">
					<span class="text-sm text-muted-foreground select-none">&gt;</span>
					<Input
						id="password"
						type="password"
						bind:value={password}
						placeholder="enter password"
						required
						autocomplete="current-password"
						class="border-none bg-transparent p-1 font-mono text-sm shadow-none placeholder:text-muted-foreground/40 focus-visible:ring-0"
					/>
				</div>
				<div class="h-px bg-border"></div>
			</div>

			<Button type="submit" disabled={loading} variant="ghost" size="sm">
				{loading ? '_ signing in...' : '_ sign in →'}
			</Button>
		</form>
	</div>
</div>
