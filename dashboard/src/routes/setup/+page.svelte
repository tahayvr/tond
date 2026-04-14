<script lang="ts">
	import { goto } from '$app/navigation';
	import { setupAdmin } from '$lib/api';
	import { setAuthenticated } from '$lib/auth.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import TondLogo from '$lib/components/brand/tond-logo.svelte';

	let email = $state('');
	let password = $state('');
	let confirmPassword = $state('');
	let error = $state('');
	let loading = $state(false);

	const requirements = $derived([
		{ label: 'at least 12 characters', met: password.length >= 12 },
		{ label: 'one uppercase letter (A–Z)', met: /[A-Z]/.test(password) },
		{ label: 'one lowercase letter (a–z)', met: /[a-z]/.test(password) },
		{ label: 'one number (0–9)', met: /[0-9]/.test(password) },
		{ label: 'one special character (!@#$…)', met: /[^A-Za-z0-9]/.test(password) }
	]);

	const passwordValid = $derived(requirements.every((r) => r.met));

	async function handleSetup(e: Event) {
		e.preventDefault();
		error = '';

		if (!passwordValid) {
			error = 'password does not meet all requirements';
			return;
		}
		if (password !== confirmPassword) {
			error = 'passwords do not match';
			return;
		}

		loading = true;
		try {
			const result = await setupAdmin(email, password);
			setAuthenticated(result.user);
			await goto('/');
		} catch (err: any) {
			error = err.message || 'setup failed';
		} finally {
			loading = false;
		}
	}
</script>

<div class="flex min-h-[80vh] flex-col items-center justify-center">
	<div class="w-full max-w-lg space-y-8 font-mono">
		<div class="flex flex-col items-center gap-4">
			<TondLogo class="h-8 opacity-80" />
			<p class="text-xs text-muted-foreground">one-time admin setup</p>
		</div>

		{#if error}
			<p class="text-sm text-destructive">! {error}</p>
		{/if}

		<form onsubmit={handleSetup} class="space-y-6">
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
						placeholder="strong password"
						required
						minlength={12}
						autocomplete="new-password"
						class="border-none bg-transparent p-1 font-mono text-sm shadow-none placeholder:text-muted-foreground/40 focus-visible:ring-0"
					/>
				</div>
				<div class="h-px bg-border"></div>
				{#if password.length > 0}
					<ul class="mt-2 space-y-0.5">
						{#each requirements as req (req.label)}
							<li
								class="text-xs {req.met
									? 'text-green-600 dark:text-green-500'
									: 'text-muted-foreground'}"
							>
								{req.met ? '✓' : '○'}
								{req.label}
							</li>
						{/each}
					</ul>
				{/if}
			</div>

			<div class="space-y-1">
				<label for="confirm-password" class="block text-xs text-muted-foreground"
					>confirm password</label
				>
				<div class="flex items-center gap-2">
					<span class="text-sm text-muted-foreground select-none">&gt;</span>
					<Input
						id="confirm-password"
						type="password"
						bind:value={confirmPassword}
						placeholder="repeat password"
						required
						minlength={12}
						autocomplete="new-password"
						class="border-none bg-transparent p-1 font-mono text-sm shadow-none placeholder:text-muted-foreground/40 focus-visible:ring-0"
					/>
				</div>
				<div class="h-px bg-border"></div>
			</div>

			<Button type="submit" disabled={loading} variant="ghost" size="sm">
				{loading ? '_ creating account...' : '_ create admin account →'}
			</Button>
		</form>
	</div>
</div>
