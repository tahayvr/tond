<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';

	interface Props {
		phase: 'idle' | 'setup' | 'done' | 'disable';
		enabled: boolean;
		setup: { qr_code: string; secret: string } | null;
		totpCode: string;
		disablePassword: string;
		disableCode: string;
		backupCodes: string[];
		saving: boolean;
		error: string;
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
		totpCode = $bindable(''),
		disablePassword = $bindable(''),
		disableCode = $bindable(''),
		backupCodes,
		saving,
		error,
		onEnable,
		onConfirm,
		onCancelSetup,
		onDisable,
		onCancelDisable,
		onShowDisable
	}: Props = $props();
</script>

{#if error}
	<p>{error}</p>
{/if}

{#if phase === 'idle' && !enabled}
	<Button onclick={onEnable} disabled={saving}>Enable Two-Factor Auth</Button>
{:else if phase === 'setup' && setup}
	<img alt="TOTP QR code" src="data:image/png;base64,{setup.qr_code}" />
	<p>{setup.secret}</p>
	<label>
		Enter one-time password
		<input bind:value={totpCode} />
	</label>
	<Button onclick={onConfirm} disabled={saving}>Confirm &amp; Enable</Button>
	<Button onclick={onCancelSetup}>Cancel</Button>
{:else if phase === 'done'}
	<p>Save these backup codes</p>
	{#each backupCodes as code (code)}
		<p>{code}</p>
	{/each}
	<Button onclick={onShowDisable}>Disable 2FA</Button>
{:else if enabled && phase !== 'disable'}
	<Button onclick={onShowDisable}>Disable 2FA</Button>
{:else if phase === 'disable'}
	<label>
		Current Password
		<input bind:value={disablePassword} />
	</label>
	<label>
		Authenticator Code
		<input bind:value={disableCode} />
	</label>
	<Button onclick={onDisable} disabled={saving}>Disable 2FA</Button>
	<Button onclick={onCancelDisable}>Cancel</Button>
{/if}
