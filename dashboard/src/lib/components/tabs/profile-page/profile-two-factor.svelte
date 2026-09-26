<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import type { TotpSetup } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as InputOTP from '$lib/components/ui/input-otp/index.js';
	import { REGEXP_ONLY_DIGITS } from 'bits-ui';
	import ShieldCheck from '~icons/ri/shield-check-line';

	interface Props {
		phase: 'idle' | 'password' | 'setup' | 'done' | 'disable';
		enabled: boolean;
		setup: TotpSetup | null;
		setupPassword: string;
		totpCode: string;
		disablePassword: string;
		disableCode: string;
		backupCodes: string[];
		saving: boolean;
		error: string;
		onStart: () => void;
		onEnable: () => void;
		onConfirm: () => void;
		onCancelSetup: () => void;
		onDisable: () => void;
		onCancelDisable: () => void;
		onShowDisable: () => void;
	}

	let {
		phase,
		enabled,
		setup,
		setupPassword = $bindable(''),
		totpCode = $bindable(''),
		disablePassword = $bindable(''),
		disableCode = $bindable(''),
		backupCodes,
		saving,
		error,
		onStart,
		onEnable,
		onConfirm,
		onCancelSetup,
		onDisable,
		onCancelDisable,
		onShowDisable
	}: Props = $props();
</script>

<div class="rounded-lg border border-border bg-card p-4">
	<div class="mb-4 flex items-center gap-2">
		<ShieldCheck class="h-4 w-4 text-muted-foreground" />
		<span class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
			TWO_FACTOR_AUTH
		</span>
	</div>

	<InlineError message={error} />

	{#if phase === 'idle' && !enabled}
		<!-- Not set up -->
		<p class="mb-4 text-sm text-muted-foreground">
			Two-factor authentication is not enabled. Add a second factor to protect your account.
		</p>
		<Button onclick={onStart} disabled={saving}>Enable Two-Factor Auth</Button>
	{:else if phase === 'password'}
		<!-- Re-authenticate before a new authenticator can be enrolled -->
		<p class="mb-4 text-sm text-muted-foreground">
			Confirm your password to set up two-factor authentication.
		</p>
		<form
			class="max-w-sm space-y-4"
			onsubmit={(e) => {
				e.preventDefault();
				onEnable();
			}}
		>
			<div class="grid gap-2">
				<Label for="setup-pw" class="font-mono text-[10px] text-muted-foreground uppercase">
					Current Password
				</Label>
				<Input
					id="setup-pw"
					type="password"
					bind:value={setupPassword}
					autocomplete="current-password"
					class="border-border bg-card font-mono text-sm"
					required
				/>
			</div>
			<div class="flex gap-2">
				<Button type="submit" disabled={saving || !setupPassword}>
					{saving ? 'Generating…' : 'Continue'}
				</Button>
				<Button type="button" variant="ghost" onclick={onCancelSetup}>Cancel</Button>
			</div>
		</form>
	{:else if phase === 'setup' && setup}
		<!-- Scan QR, confirm first code -->
		<p class="mb-4 text-sm text-muted-foreground">
			Scan the QR code with your authenticator app, then enter the 6-digit code to confirm.
		</p>

		<div class="mb-4 space-y-3">
			<img
				src="data:image/png;base64,{setup.qr_code}"
				alt="TOTP QR code"
				class="rounded-md border border-border"
				width="180"
				height="180"
			/>
			<div>
				<p class="mb-1 font-mono text-[10px] text-muted-foreground uppercase">
					Manual entry key
				</p>
				<code class="rounded bg-muted px-2 py-1 font-mono text-xs tracking-widest select-all">
					{setup.secret}
				</code>
			</div>
		</div>

		<div class="space-y-4">
			<div>
				<p class="mb-3 font-mono text-[10px] text-muted-foreground uppercase">
					Enter code to confirm
				</p>
				<InputOTP.Root
					maxlength={6}
					pattern={REGEXP_ONLY_DIGITS}
					bind:value={totpCode}
					autocomplete="one-time-code"
				>
					{#snippet children({ cells })}
						<InputOTP.Group>
							{#each cells.slice(0, 3) as cell (cell)}
								<InputOTP.Slot {cell} />
							{/each}
						</InputOTP.Group>
						<InputOTP.Separator />
						<InputOTP.Group>
							{#each cells.slice(3, 6) as cell (cell)}
								<InputOTP.Slot {cell} />
							{/each}
						</InputOTP.Group>
					{/snippet}
				</InputOTP.Root>
			</div>

			<div class="flex gap-2">
				<Button onclick={onConfirm} disabled={saving || totpCode.length !== 6}>
					{saving ? 'Verifying…' : 'Confirm & Enable'}
				</Button>
				<Button variant="ghost" onclick={onCancelSetup}>Cancel</Button>
			</div>
		</div>
	{:else if phase === 'done'}
		<!-- Just activated — show backup codes -->
		<div
			class="mb-6 rounded-md border border-warn/30 bg-warn/10 p-4 text-sm text-warn"
		>
			<p class="mb-1 font-semibold">Save these backup codes</p>
			<p class="text-xs">
				Each code can only be used once. Store them somewhere safe — you won't see them again.
			</p>
		</div>
		<p class="mb-4 text-xs text-muted-foreground">
			For your security, every other session on your account has been signed out.
		</p>

		<div class="mb-6 grid grid-cols-2 gap-2">
			{#each backupCodes as code (code)}
				<code
					class="rounded border border-border bg-muted px-3 py-1.5 text-center font-mono text-xs tracking-widest select-all"
				>
					{code}
				</code>
			{/each}
		</div>

		<div class="flex items-center gap-3">
			<ShieldCheck class="h-4 w-4 text-success" />
			<span class="font-mono text-xs text-success">2FA ENABLED</span>
			<Button variant="outline" size="sm" class="ml-auto" onclick={onShowDisable}>
				Disable 2FA
			</Button>
		</div>
	{:else if enabled && phase !== 'disable'}
		<!-- Enabled, steady state -->
		<div class="mb-4 flex items-center gap-3">
			<ShieldCheck class="h-4 w-4 text-success" />
			<span class="font-mono text-xs text-success">2FA ENABLED</span>
		</div>
		<p class="mb-4 text-sm text-muted-foreground">
			Your account is protected with a time-based one-time password.
		</p>
		<Button variant="outline" size="sm" onclick={onShowDisable}>Disable 2FA</Button>
	{:else if phase === 'disable'}
		<!-- Disable flow -->
		<p class="mb-4 text-sm text-muted-foreground">
			Enter your current password and a 6-digit code from your authenticator app to disable 2FA.
			Your other sessions will be signed out.
		</p>

		<form
			class="max-w-sm space-y-4"
			onsubmit={(e) => {
				e.preventDefault();
				onDisable();
			}}
		>
			<div class="grid gap-2">
				<Label for="disable-pw" class="font-mono text-[10px] text-muted-foreground uppercase">
					Current Password
				</Label>
				<Input
					id="disable-pw"
					type="password"
					bind:value={disablePassword}
					autocomplete="current-password"
					class="border-border bg-card font-mono text-sm"
					required
				/>
			</div>

			<div class="grid gap-2">
				<p class="font-mono text-[10px] text-muted-foreground uppercase">Authenticator Code</p>
				<InputOTP.Root
					maxlength={6}
					pattern={REGEXP_ONLY_DIGITS}
					bind:value={disableCode}
					autocomplete="one-time-code"
				>
					{#snippet children({ cells })}
						<InputOTP.Group>
							{#each cells.slice(0, 3) as cell (cell)}
								<InputOTP.Slot {cell} />
							{/each}
						</InputOTP.Group>
						<InputOTP.Separator />
						<InputOTP.Group>
							{#each cells.slice(3, 6) as cell (cell)}
								<InputOTP.Slot {cell} />
							{/each}
						</InputOTP.Group>
					{/snippet}
				</InputOTP.Root>
			</div>

			<div class="flex gap-2">
				<Button
					type="submit"
					variant="destructive"
					disabled={saving || !disablePassword || disableCode.length !== 6}
				>
					{saving ? 'Disabling…' : 'Disable 2FA'}
				</Button>
				<Button type="button" variant="ghost" onclick={onCancelDisable}>Cancel</Button>
			</div>
		</form>
	{/if}
</div>
