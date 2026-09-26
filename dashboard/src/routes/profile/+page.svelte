<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import {
		changePassword,
		setup2FA,
		confirm2FA,
		disable2FA,
		listSessions,
		revokeSession,
		revokeAllSessions,
		type ApiToken,
		type SessionInfo,
		type TotpSetup,
		type User
	} from '$lib/api';
	import { getUser, setAuthenticated } from '$lib/auth.svelte';
	import { page } from '$app/state';
	import { passwordProblem, ROLE_LABELS } from '$lib/access';
	import TokenList from '$lib/components/tokens/token-list.svelte';
	import { untrack } from 'svelte';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import LinkTabs from '$lib/components/nav/link-tabs.svelte';
	import { PROFILE_TABS, tabFromUrl, tabHref, type ProfileTab } from '$lib/tabs';
	import { confirmAction } from '$lib/confirm.svelte';
	import {
		acknowledgeBackupCodes,
		getPendingBackupCodes,
		setPendingBackupCodes
	} from '$lib/backup-codes.svelte';
	import ProfilePassword from '$lib/components/tabs/profile-page/profile-password.svelte';
	import ProfileTwoFactor from '$lib/components/tabs/profile-page/profile-two-factor.svelte';
	import ProfileSessions from '$lib/components/tabs/profile-page/profile-sessions.svelte';
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
		const problem = passwordProblem(newPassword);
		if (problem) {
			pwError = problem;
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
	let totpPhase = $state<'idle' | 'password' | 'setup' | 'done' | 'disable'>('idle');
	let totpSetup = $state<TotpSetup | null>(null);
	let totpSetupPassword = $state('');
	let totpCode = $state('');
	let totpDisablePassword = $state('');
	let totpDisableCode = $state('');
	let totpError = $state('');
	let totpSaving = $state(false);
	// Freshly issued backup codes stay until the user confirms they saved them.
	let backupCodes = $derived(getPendingBackupCodes());
	let tab = $derived<ProfileTab>(tabFromUrl(page.url, PROFILE_TABS, 'password'));
	let shownPhase = $derived(backupCodes.length > 0 ? 'done' : totpPhase);
	// Seeded from the freshly loaded user so enrolled users are offered "disable".
	let twoFaEnabled = $state(
		untrack(() => (data.me as User | undefined)?.totp_enabled ?? getUser()?.totp_enabled ?? false)
	);

	/** Keep the shared auth user in sync so other screens see the new 2FA state. */
	function syncUserTotp(enabled: boolean) {
		const current = getUser();
		if (current) setAuthenticated({ ...current, totp_enabled: enabled });
	}

	function handleStart2FA() {
		totpError = '';
		totpSetupPassword = '';
		totpPhase = 'password';
	}

	async function handleEnable2FA() {
		totpError = '';
		totpSaving = true;
		try {
			totpSetup = await setup2FA(totpSetupPassword);
			totpSetupPassword = '';
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
			setPendingBackupCodes(result.backup_codes);
			twoFaEnabled = true;
			syncUserTotp(true);
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
			syncUserTotp(false);
			totpPhase = 'idle';
			totpDisablePassword = '';
			totpDisableCode = '';
			acknowledgeBackupCodes();
		} catch (e: any) {
			totpError = e.message;
		} finally {
			totpSaving = false;
		}
	}

	function cancelSetup() {
		totpPhase = 'idle';
		totpSetup = null;
		totpSetupPassword = '';
		totpCode = '';
		totpError = '';
	}

	function cancelDisable() {
		totpPhase = 'idle';
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

	function handleRevokeSession(id: string) {
		sessionsError = '';
		return confirmAction({
			title: 'Sign out this session?',
			description: 'That browser or device is signed out immediately and must sign in again.',
			confirmLabel: 'Sign out session',
			pendingLabel: 'Signing out…',
			onconfirm: async () => {
				revokingId = id;
				try {
					await revokeSession(id);
					sessions = sessions.filter((s) => s.id !== id);
				} finally {
					revokingId = null;
				}
			}
		});
	}

	function handleRevokeAll() {
		sessionsError = '';
		return confirmAction({
			title: 'Sign out all other sessions?',
			description:
				'Every other browser and device signed in to your account is signed out. This one stays signed in.',
			confirmLabel: 'Sign out others',
			pendingLabel: 'Signing out…',
			onconfirm: async () => {
				revokingAll = true;
				try {
					await revokeAllSessions();
					sessions = sessions.filter((s) => s.is_current);
				} finally {
					revokingAll = false;
				}
			}
		});
	}

	// Load sessions fresh when the Sessions tab opens.
	$effect(() => {
		if (tab === 'sessions') untrack(loadSessions);
	});

	// Warn before leaving the page with unsaved backup codes.
	$effect(() => {
		if (backupCodes.length === 0) return;
		const warn = (e: BeforeUnloadEvent) => e.preventDefault();
		window.addEventListener('beforeunload', warn);
		return () => window.removeEventListener('beforeunload', warn);
	});

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<PageTitle title="Profile" />

<div class="space-y-6">
	<div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
		<div class="min-w-0">
			<h1 class="text-2xl font-extrabold sm:text-3xl">Profile</h1>
			<p class="truncate text-sm text-muted-foreground">{user?.email ?? ''}</p>
		</div>
		<Badge variant="outline" class="text-xs">
			{user ? (ROLE_LABELS[user.role] ?? user.role) : ''}
		</Badge>
	</div>

	<LinkTabs
		label="Profile sections"
		tabs={PROFILE_TABS}
		active={tab}
		href={(id) => tabHref(page.url.pathname, id, 'password')}
	/>

	<div class="pt-2">
		{#if tab === 'password'}
			<ProfilePassword
				bind:currentPassword
				bind:newPassword
				bind:confirmPassword
				saving={pwSaving}
				saved={pwSaved}
				error={pwError}
				onSubmit={handleChangePassword}
			/>
		{:else if tab === '2fa'}
			<ProfileTwoFactor
				phase={shownPhase}
				enabled={twoFaEnabled}
				setup={totpSetup}
				bind:setupPassword={totpSetupPassword}
				bind:totpCode
				bind:disablePassword={totpDisablePassword}
				bind:disableCode={totpDisableCode}
				{backupCodes}
				saving={totpSaving}
				error={totpError}
				onStart={handleStart2FA}
				onEnable={handleEnable2FA}
				onConfirm={handleConfirm2FA}
				onCancelSetup={cancelSetup}
				onDisable={handleDisable2FA}
				onCancelDisable={cancelDisable}
				onShowDisable={() => {
					totpPhase = 'disable';
					totpError = '';
				}}
				onAcknowledge={() => {
					acknowledgeBackupCodes();
					totpPhase = 'idle';
				}}
				account={user?.email}
			/>
		{:else if tab === 'sessions'}
			<ProfileSessions
				{sessions}
				loading={sessionsLoading}
				error={sessionsError}
				{revokingId}
				{revokingAll}
				onRevoke={handleRevokeSession}
				onRevokeAll={handleRevokeAll}
			/>
		{:else if tab === 'tokens'}
			<TokenList initialTokens={(data.tokens as ApiToken[] | undefined) ?? []} />
		{/if}
	</div>
</div>
