<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import { verify2FA } from '$lib/api';
	import { setAuthenticated } from '$lib/auth.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as InputOTP from '$lib/components/ui/input-otp/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import TondLogo from '$lib/components/brand/tond-logo.svelte';
	import { REGEXP_ONLY_DIGITS } from 'bits-ui';
	import { page } from '$app/state';

	let otpValue = $state('');
	let backupCode = $state('');
	let useBackup = $state(false);
	let error = $state('');
	let loading = $state(false);

	let code = $derived(useBackup ? backupCode : otpValue);
	let canSubmit = $derived(!loading && (useBackup ? backupCode.length > 0 : otpValue.length === 6));

	async function handleVerify(e: Event) {
		e.preventDefault();
		error = '';
		loading = true;

		try {
			const result = await verify2FA(code);
			setAuthenticated(result.user);
		} catch (err: any) {
			error = err.message || 'Verification failed';
		} finally {
			loading = false;
		}
	}
</script>

<PageTitle title="Two-factor sign in" />

<div class="flex min-h-[80vh] flex-col items-center justify-center">
	<div class="w-full max-w-lg space-y-8 font-mono">
		<div class="flex flex-col items-center gap-4">
			<TondLogo class="h-8 opacity-80" />
			<p class="text-xs text-muted-foreground">operator verification</p>
		</div>

		{#if error}
			<p class="text-sm text-destructive">! {error}</p>
		{/if}

		<form onsubmit={handleVerify} class="space-y-6">
			<div class="space-y-1">
				<p class="block text-xs text-muted-foreground">
					{useBackup ? 'backup code' : 'authenticator code'}
				</p>
				<p class="text-xs text-muted-foreground">
					{useBackup
						? 'enter one of your 16-character backup codes'
						: 'enter the 6-digit code from your authenticator app'}
				</p>
			</div>

			{#if !useBackup}
				<div class="space-y-2">
					<div class="flex items-center gap-2">
						<span class="text-sm text-muted-foreground select-none">&gt;</span>
						<InputOTP.Root
							maxlength={6}
							pattern={REGEXP_ONLY_DIGITS}
							bind:value={otpValue}
							autocomplete="one-time-code"
							class="gap-0"
						>
							{#snippet children({ cells })}
								<InputOTP.Group>
									{#each cells.slice(0, 3) as cell (cell)}
										<InputOTP.Slot
											{cell}
											class="size-10 rounded-none border border-b border-border bg-transparent font-mono text-sm first:rounded-none  last:rounded-none"
										/>
									{/each}
								</InputOTP.Group>
								<InputOTP.Separator class="px-2 text-muted-foreground">-</InputOTP.Separator>
								<InputOTP.Group>
									{#each cells.slice(3, 6) as cell (cell)}
										<InputOTP.Slot
											{cell}
											class="size-10 rounded-none  border-b border-border bg-transparent font-mono text-sm first:rounded-none  last:rounded-none"
										/>
									{/each}
								</InputOTP.Group>
							{/snippet}
						</InputOTP.Root>
					</div>
					<div class="h-px bg-border"></div>
				</div>
			{:else}
				<div class="space-y-1">
					<label for="backup-code" class="block text-xs text-muted-foreground">backup-code</label>
					<div class="flex items-center gap-2">
						<span class="text-sm text-muted-foreground select-none">&gt;</span>
						<Input
							id="backup-code"
							type="text"
							bind:value={backupCode}
							placeholder="xxxxxxxxxxxxxxxx"
							maxlength={16}
							autocomplete="off"
							class="border-none bg-transparent p-1 font-mono text-sm shadow-none placeholder:text-muted-foreground focus-visible:ring-2"
						/>
					</div>
					<div class="h-px bg-border"></div>
				</div>
			{/if}

			<Button type="submit" disabled={!canSubmit} variant="ghost" size="sm">
				{loading ? '_ verifying...' : '_ verify →'}
			</Button>

			<button
				type="button"
				class="text-left text-xs text-muted-foreground hover:text-foreground"
				onclick={() => {
					useBackup = !useBackup;
					error = '';
					otpValue = '';
					backupCode = '';
				}}
			>
				{useBackup ? 'use authenticator app instead' : 'use a backup code instead'}
			</button>

			<a
				href={`/login${page.url.search}`}
				class="block text-xs text-muted-foreground hover:text-foreground"
			>
				← back to sign in
			</a>
		</form>
	</div>
</div>
