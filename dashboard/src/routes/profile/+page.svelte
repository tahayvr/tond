<script lang="ts">
	import {
		changePassword,
		setup2FA,
		confirm2FA,
		disable2FA,
		listSessions,
		revokeSession,
		revokeAllSessions,
		type SessionInfo,
		type TotpSetup
	} from '$lib/api';
	import { getUser } from '$lib/auth.svelte';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import ProfilePassword from '$lib/components/tabs/profile-page/profile-password.svelte';
	import ProfileTwoFactor from '$lib/components/tabs/profile-page/profile-two-factor.svelte';
	import ProfileSessions from '$lib/components/tabs/profile-page/profile-sessions.svelte';
	import KeyRound from '@lucide/svelte/icons/key-round';
	import ShieldCheck from '@lucide/svelte/icons/shield-check';
	import Monitor from '@lucide/svelte/icons/monitor';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const user = $derived(getUser());

	// ── Change Password ───────────────────────────────────────────────────────
	let currentPassword = $state('');
	let newPassword = $state('');
	let confirmPassword = $state('');
	let pwSaving = $state(false);
	let pwSaved = $state(false);
	let pwError = $state('');

	async function handleChangePassword() {
		pwError = '';
		pwSaved = false;
		if (newPassword !== confirmPassword) {
			pwError = 'New passwords do not match';
			return;
		}
		if (newPassword.length < 8) {
			pwError = 'New password must be at least 8 characters';
			return;
		}
		pwSaving = true;
		try {
			await changePassword(currentPassword, newPassword);
			pwSaved = true;
			currentPassword = '';
			newPassword = '';
			confirmPassword = '';
			setTimeout(() => (pwSaved = false), 4000);
		} catch (e: any) {
			pwError = e.message;
		} finally {
			pwSaving = false;
		}
	}

	// ── TOTP 2FA ──────────────────────────────────────────────────────────────
	let totpPhase = $state<'idle' | 'setup' | 'done' | 'disable'>('idle');
	let totpSetup = $state<TotpSetup | null>(null);
	let totpCode = $state('');
	let totpDisablePassword = $state('');
	let totpDisableCode = $state('');
	let totpError = $state('');
	let totpSaving = $state(false);
	let backupCodes = $state<string[]>([]);
	let twoFaEnabled = $state(false);

	async function handleEnable2FA() {
		totpError = '';
		totpSaving = true;
		try {
			totpSetup = await setup2FA();
			totpPhase = 'setup';
		} catch (e: any) {
			totpError = e.message;
		} finally {
			totpSaving = false;
		}
	}

	async function handleConfirm2FA() {
		totpError = '';
		totpSaving = true;
		try {
			const result = await confirm2FA(totpCode);
			backupCodes = result.backup_codes;
			twoFaEnabled = true;
			totpPhase = 'done';
			totpCode = '';
		} catch (e: any) {
			totpError = e.message;
		} finally {
			totpSaving = false;
		}
	}

	async function handleDisable2FA() {
		totpError = '';
		totpSaving = true;
		try {
			await disable2FA(totpDisablePassword, totpDisableCode);
			twoFaEnabled = false;
			totpPhase = 'idle';
			totpDisablePassword = '';
			totpDisableCode = '';
			backupCodes = [];
		} catch (e: any) {
			totpError = e.message;
		} finally {
			totpSaving = false;
		}
	}

	function cancelSetup() {
		totpPhase = 'idle';
		totpSetup = null;
		totpCode = '';
		totpError = '';
	}

	function cancelDisable() {
		totpPhase = twoFaEnabled ? 'idle' : 'idle';
		totpDisablePassword = '';
		totpDisableCode = '';
		totpError = '';
	}

	// ── Sessions ──────────────────────────────────────────────────────────────
	let sessions = $derived(data.sessions as SessionInfo[]);
	let sessionsLoading = $state(false);
	let sessionsError = $state('');
	let revokingId = $state<string | null>(null);
	let revokingAll = $state(false);

	async function loadSessions() {
		sessionsLoading = true;
		sessionsError = '';
		try {
			sessions = await listSessions();
		} catch (e: any) {
			sessionsError = e.message;
		} finally {
			sessionsLoading = false;
		}
	}

	async function handleRevokeSession(id: string) {
		revokingId = id;
		try {
			await revokeSession(id);
			sessions = sessions.filter((s) => s.id !== id);
		} catch (e: any) {
			sessionsError = e.message;
		} finally {
			revokingId = null;
		}
	}

	async function handleRevokeAll() {
		revokingAll = true;
		sessionsError = '';
		try {
			await revokeAllSessions();
			sessions = sessions.filter((s) => s.is_current);
		} catch (e: any) {
			sessionsError = e.message;
		} finally {
			revokingAll = false;
		}
	}

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<div class="flex h-14 items-center justify-between">
		<div>
			<h2 class="text-3xl font-extrabold uppercase">Profile</h2>
			<p class="font-mono text-xs tracking-wider text-muted-foreground/70">
				{user?.email ?? ''}
			</p>
		</div>
		<Badge variant="outline" class="font-mono text-xs uppercase">{user?.role ?? ''}</Badge>
	</div>

	<Tabs.Root value="password">
		<Tabs.List class="mb-6">
			<Tabs.Trigger value="password">
				<KeyRound class="mr-1.5 h-3.5 w-3.5" />
				Password
			</Tabs.Trigger>
			<Tabs.Trigger value="2fa">
				<ShieldCheck class="mr-1.5 h-3.5 w-3.5" />
				Two-Factor Auth
			</Tabs.Trigger>
			<Tabs.Trigger value="sessions" onclick={loadSessions}>
				<Monitor class="mr-1.5 h-3.5 w-3.5" />
				Sessions
			</Tabs.Trigger>
		</Tabs.List>

		<Tabs.Content value="password">
			<ProfilePassword
				bind:currentPassword
				bind:newPassword
				bind:confirmPassword
				saving={pwSaving}
				saved={pwSaved}
				error={pwError}
				onSubmit={handleChangePassword}
			/>
		</Tabs.Content>

		<Tabs.Content value="2fa">
			<ProfileTwoFactor
				phase={totpPhase}
				enabled={twoFaEnabled}
				setup={totpSetup}
				bind:totpCode
				bind:disablePassword={totpDisablePassword}
				bind:disableCode={totpDisableCode}
				{backupCodes}
				saving={totpSaving}
				error={totpError}
				onEnable={handleEnable2FA}
				onConfirm={handleConfirm2FA}
				onCancelSetup={cancelSetup}
				onDisable={handleDisable2FA}
				onCancelDisable={cancelDisable}
				onShowDisable={() => {
					totpPhase = 'disable';
					totpError = '';
				}}
			/>
		</Tabs.Content>

		<Tabs.Content value="sessions">
			<ProfileSessions
				{sessions}
				loading={sessionsLoading}
				error={sessionsError}
				{revokingId}
				{revokingAll}
				onRevoke={handleRevokeSession}
				onRevokeAll={handleRevokeAll}
			/>
		</Tabs.Content>
	</Tabs.Root>
</div>
