<script lang="ts">
	import {
		createPasswordLink,
		deleteUser,
		listUserSessions,
		listUsers,
		revokeUserSession,
		revokeUserSessions,
		updateUser,
		type AdminUser,
		type PasswordLink,
		type Role,
		type SessionInfo
	} from '$lib/api';
	import { ROLE_LABELS, relativeTime } from '$lib/access';
	import { can } from '$lib/auth.svelte';
	import { cn } from '$lib/utils';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import ConfirmDialog from '$lib/components/dialogs/confirm-dialog.svelte';
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import InviteUserDialog from '$lib/components/users/invite-user-dialog.svelte';
	import PasswordLinkView from '$lib/components/users/password-link-view.svelte';
	import UserAddIcon from '~icons/ri/user-add-line';
	import TeamIcon from '~icons/ri/team-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let users = $derived<AdminUser[]>(data.users as AdminUser[]);
	let error = $state('');
	let inviteOpen = $state(false);

	async function refresh() {
		try {
			users = await listUsers();
		} catch (e: any) {
			error = e.message;
		}
	}

	// ── Confirmed actions (role change, disable/enable) ──
	interface Pending {
		title: string;
		description: string;
		confirmLabel: string;
		destructive: boolean;
		run: () => Promise<void>;
	}
	let pending = $state<Pending | null>(null);
	let pendingBusy = $state(false);
	let pendingError = $state('');

	function ask(p: Pending) {
		pendingError = '';
		pending = p;
	}

	async function runPending() {
		if (!pending) return;
		pendingBusy = true;
		pendingError = '';
		try {
			await pending.run();
			pending = null;
			await refresh();
		} catch (e: any) {
			pendingError = e.message;
		} finally {
			pendingBusy = false;
		}
	}

	function askRole(u: AdminUser, role: Role) {
		if (role === u.role) return;
		ask({
			title: `Make ${u.email} ${ROLE_LABELS[role].toLowerCase()}?`,
			description: `${u.email} will be signed out everywhere and gets the new permissions on their next sign-in. Their API tokens are narrowed to the new role right away.`,
			confirmLabel: `Change to ${ROLE_LABELS[role]}`,
			destructive: false,
			run: async () => {
				await updateUser(u.id, { role });
			}
		});
	}

	function askDisable(u: AdminUser) {
		const disabling = !u.disabled;
		ask({
			title: disabling ? `Disable ${u.email}?` : `Enable ${u.email}?`,
			description: disabling
				? 'They are signed out everywhere, can’t sign in, and their API tokens stop working until you enable the account again.'
				: 'They can sign in again and their unexpired API tokens start working again.',
			confirmLabel: disabling ? 'Disable account' : 'Enable account',
			destructive: disabling,
			run: async () => {
				await updateUser(u.id, { disabled: disabling });
			}
		});
	}

	// ── Delete ──
	let deleteTarget = $state<AdminUser | null>(null);
	let deleteOpen = $state(false);

	// ── Password links ──
	let linkBusyId = $state<string | null>(null);
	let link = $state<PasswordLink | null>(null);

	async function handleLink(u: AdminUser) {
		linkBusyId = u.id;
		error = '';
		try {
			link = await createPasswordLink(u.id);
		} catch (e: any) {
			error = e.message;
		} finally {
			linkBusyId = null;
		}
	}

	// ── Sessions ──
	let sessionsUser = $state<AdminUser | null>(null);
	let sessions = $state<SessionInfo[]>([]);
	let sessionsLoading = $state(false);
	let sessionsError = $state('');
	let sessionBusy = $state<string | null>(null);

	async function openSessions(u: AdminUser) {
		sessionsUser = u;
		sessions = [];
		sessionsError = '';
		sessionsLoading = true;
		try {
			sessions = await listUserSessions(u.id);
		} catch (e: any) {
			sessionsError = e.message;
		} finally {
			sessionsLoading = false;
		}
	}

	async function revokeOne(s: SessionInfo) {
		if (!sessionsUser) return;
		sessionBusy = s.id;
		sessionsError = '';
		try {
			await revokeUserSession(sessionsUser.id, s.id);
			sessions = sessions.filter((x) => x.id !== s.id);
			await refresh();
		} catch (e: any) {
			sessionsError = e.message;
		} finally {
			sessionBusy = null;
		}
	}

	async function revokeAll() {
		if (!sessionsUser) return;
		sessionBusy = 'all';
		sessionsError = '';
		try {
			await revokeUserSessions(sessionsUser.id);
			sessions = sessions.filter((s) => s.is_current);
			await refresh();
		} catch (e: any) {
			sessionsError = e.message;
		} finally {
			sessionBusy = null;
		}
	}

	function shortAgent(ua: string | null) {
		if (!ua) return 'Unknown device';
		if (ua.includes('Firefox')) return 'Firefox';
		if (ua.includes('Edg/')) return 'Edge';
		if (ua.includes('Chrome')) return 'Chrome';
		if (ua.includes('Safari')) return 'Safari';
		return ua.slice(0, 40);
	}

	function status(u: AdminUser): { label: string; class: string } {
		if (u.disabled) return { label: 'Disabled', class: 'border-destructive/40 text-destructive' };
		if (!u.password_set) return { label: 'Invited', class: 'border-warn/40 text-warn' };
		return { label: 'Active', class: 'border-success/40 text-success' };
	}

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<PageTitle title="Users" />

<div class="space-y-6">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-extrabold sm:text-3xl">Users</h1>
			<p class="text-sm text-muted-foreground">
				Who can sign in, and what they can do. Admins can do everything; deployers ship code;
				viewers look.
			</p>
		</div>
		{#if can('users:admin')}
			<Button size="sm" onclick={() => (inviteOpen = true)}>
				<UserAddIcon class="mr-1 size-4" /> Invite user
			</Button>
		{/if}
	</div>

	{#if error}
		<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
			{error}
		</div>
	{/if}

	{#if users.length === 0}
		<div class="rounded-lg border border-dashed border-border py-16 text-center">
			<TeamIcon class="mx-auto mb-2 size-10 text-muted-foreground" />
			<p class="font-medium">No users yet</p>
		</div>
	{:else}
		<ul class="divide-y divide-border rounded-lg border border-border bg-card">
			{#each users as u (u.id)}
				{@const st = status(u)}
				<li class="flex flex-col gap-3 p-4 lg:flex-row lg:items-center lg:justify-between">
					<div class="min-w-0 space-y-1">
						<div class="flex flex-wrap items-center gap-2">
							<span class="truncate font-medium">{u.email}</span>
							{#if u.is_you}<Badge variant="secondary" class="text-[10px]">you</Badge>{/if}
							<Badge variant="outline" class={cn('text-[10px]', st.class)}>{st.label}</Badge>
							{#if u.totp_enabled}<Badge variant="outline" class="text-[10px]">2FA</Badge>{/if}
						</div>
						<p class="text-xs text-muted-foreground">
							{#if u.password_set}
								Last sign-in {relativeTime(u.last_login_at)}
							{:else}
								Hasn't set a password yet
							{/if}
							· {u.active_sessions} active session{u.active_sessions === 1 ? '' : 's'}
						</p>
					</div>
					<div class="flex flex-wrap items-center gap-2">
						<Select.Root
							type="single"
							value={u.role}
							disabled={u.is_you}
							onValueChange={(v) => askRole(u, v as Role)}
						>
							<Select.Trigger
								class="h-8 w-32"
								aria-label={`Role of ${u.email}`}
								title={u.is_you ? "You can't change your own role" : ''}
							>
								{ROLE_LABELS[u.role] ?? u.role}
							</Select.Trigger>
							<Select.Content>
								{#each ['admin', 'deployer', 'viewer'] as const as r (r)}
									<Select.Item value={r}>{ROLE_LABELS[r]}</Select.Item>
								{/each}
							</Select.Content>
						</Select.Root>
						{#if u.active_sessions > 0}
							<Button variant="outline" size="sm" onclick={() => openSessions(u)}>Sessions</Button>
						{/if}
						{#if !u.is_you}
							<Button
								variant="outline"
								size="sm"
								onclick={() => handleLink(u)}
								disabled={linkBusyId === u.id}
							>
								{linkBusyId === u.id
									? 'Creating…'
									: u.password_set
										? 'Reset password'
										: 'New invite link'}
							</Button>
							<Button variant="outline" size="sm" onclick={() => askDisable(u)}>
								{u.disabled ? 'Enable' : 'Disable'}
							</Button>
							<Button
								variant="outline"
								size="sm"
								class="text-destructive hover:text-destructive"
								onclick={() => {
									deleteTarget = u;
									deleteOpen = true;
								}}
							>
								Delete
							</Button>
						{/if}
					</div>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<InviteUserDialog bind:open={inviteOpen} onInvited={() => refresh()} />

<AlertDialog.Root
	open={pending !== null}
	onOpenChange={(o) => {
		if (!o && !pendingBusy) pending = null;
	}}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{pending?.title}</AlertDialog.Title>
			<AlertDialog.Description>{pending?.description}</AlertDialog.Description>
		</AlertDialog.Header>
		{#if pendingError}
			<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
				{pendingError}
			</div>
		{/if}
		<AlertDialog.Footer>
			<AlertDialog.Cancel disabled={pendingBusy}>Cancel</AlertDialog.Cancel>
			<Button
				variant={pending?.destructive ? 'destructive' : 'default'}
				onclick={runPending}
				disabled={pendingBusy}
			>
				{pendingBusy ? 'Saving…' : pending?.confirmLabel}
			</Button>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

{#if deleteTarget}
	<ConfirmDialog
		bind:open={deleteOpen}
		title={`Delete ${deleteTarget.email}?`}
		description="They lose access immediately, their sessions end and their API tokens are deleted. Their past activity stays in the log."
		confirmLabel="Delete user"
		pendingLabel="Deleting…"
		typedConfirmation={deleteTarget.email}
		onconfirm={async () => {
			if (!deleteTarget) return;
			await deleteUser(deleteTarget.id);
			deleteTarget = null;
			await refresh();
		}}
	/>
{/if}

<Dialog.Root
	open={link !== null}
	onOpenChange={(o) => {
		if (!o) link = null;
	}}
>
	<Dialog.Content class="sm:max-w-lg">
		{#if link}
			<Dialog.Header>
				<Dialog.Title>
					{link.user.password_set ? 'Password reset link' : 'New invite link'} for {link.user.email}
				</Dialog.Title>
				<Dialog.Description>
					{link.user.password_set
						? 'Setting a new password signs them out everywhere. Older links stop working.'
						: 'Older invite links stop working.'}
				</Dialog.Description>
			</Dialog.Header>
			<PasswordLinkView {link} />
			<Dialog.Footer>
				<Button onclick={() => (link = null)}>Done</Button>
			</Dialog.Footer>
		{/if}
	</Dialog.Content>
</Dialog.Root>

<Dialog.Root
	open={sessionsUser !== null}
	onOpenChange={(o) => {
		if (!o) sessionsUser = null;
	}}
>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title>Sessions of {sessionsUser?.email}</Dialog.Title>
			<Dialog.Description>Browsers signed in to this account right now.</Dialog.Description>
		</Dialog.Header>
		{#if sessionsError}
			<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
				{sessionsError}
			</div>
		{/if}
		{#if sessionsLoading}
			<p class="py-6 text-center text-sm text-muted-foreground">Loading…</p>
		{:else if sessions.length === 0}
			<p class="py-6 text-center text-sm text-muted-foreground">No active sessions.</p>
		{:else}
			<ul class="max-h-80 divide-y divide-border overflow-y-auto rounded-md border border-border">
				{#each sessions as s (s.id)}
					<li class="flex items-center justify-between gap-3 p-3 text-sm">
						<div class="min-w-0">
							<p class="font-medium">
								{shortAgent(s.user_agent)}
								{#if s.is_current}<Badge variant="secondary" class="ml-1 text-[10px]"
										>this browser</Badge
									>{/if}
							</p>
							<p class="text-xs text-muted-foreground">
								{s.ip_address ?? 'unknown IP'} · active {relativeTime(
									s.last_used_at ?? s.created_at
								)}
							</p>
						</div>
						{#if !s.is_current}
							<Button
								variant="ghost"
								size="sm"
								class="text-destructive hover:text-destructive"
								onclick={() => revokeOne(s)}
								disabled={sessionBusy !== null}
							>
								{sessionBusy === s.id ? 'Revoking…' : 'Revoke'}
							</Button>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		<Dialog.Footer>
			{#if sessions.some((s) => !s.is_current)}
				<Button variant="destructive" onclick={revokeAll} disabled={sessionBusy !== null}>
					{sessionBusy === 'all' ? 'Signing out…' : 'Sign out everywhere'}
				</Button>
			{/if}
			<Button variant="outline" onclick={() => (sessionsUser = null)}>Close</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
