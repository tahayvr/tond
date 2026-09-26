<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import Mail from '~icons/ri/mail-line';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import SavedSecretField, {
		secretFieldUpdate
	} from '$lib/components/secrets/saved-secret-field.svelte';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { Integration, IntegrationConfig, EmailConfig } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let integrations = $derived((data as { integrations: Integration[] }).integrations);

	// ── Form fields ───────────────────────────────────────────────────────────
	let fromName = $state('');
	let fromAddr = $state('');
	let smtpHost = $state('');
	let smtpPort = $state('587');
	let smtpUser = $state('');
	// The SMTP password is write-only: a saved one is never shown or prefilled.
	let smtpPassword = $state('');
	let passwordSaved = $state(false);
	let passwordReplacing = $state(false);
	let smtpEncryption = $state('starttls');
	let toAddrs = $state('');

	function buildConfig(): IntegrationConfig {
		if (!fromAddr.trim()) throw new Error('From address is required');
		if (!smtpHost.trim()) throw new Error('SMTP host is required');
		if (!smtpUser.trim()) throw new Error('SMTP username is required');
		if (!toAddrs.trim()) throw new Error('At least one recipient address is required');
		const port = parseInt(smtpPort, 10);
		if (isNaN(port)) throw new Error('SMTP port must be a number');
		const recipients = toAddrs
			.split(/[\n,]+/)
			.map((a) => a.trim())
			.filter(Boolean);
		if (recipients.length === 0) throw new Error('At least one recipient address is required');
		const password = secretFieldUpdate({
			saved: passwordSaved,
			replacing: passwordReplacing,
			removed: false,
			value: smtpPassword
		});
		if (password === undefined && (!passwordSaved || passwordReplacing)) {
			throw new Error('SMTP password is required');
		}
		return {
			from_name: fromName.trim() || 'Tond',
			from_addr: fromAddr.trim(),
			smtp_host: smtpHost.trim(),
			smtp_port: port,
			smtp_user: smtpUser.trim(),
			...(password !== undefined && { smtp_password: password }),
			smtp_encryption: smtpEncryption,
			to_addrs: recipients
		};
	}

	function resetConfig() {
		fromName = '';
		fromAddr = '';
		smtpHost = '';
		smtpPort = '587';
		smtpUser = '';
		smtpPassword = '';
		passwordSaved = false;
		passwordReplacing = false;
		smtpEncryption = 'starttls';
		toAddrs = '';
	}

	function loadConfig(config: IntegrationConfig, savedSecrets: string[]) {
		const c = config as EmailConfig;
		fromName = c.from_name ?? '';
		fromAddr = c.from_addr;
		smtpHost = c.smtp_host;
		smtpPort = String(c.smtp_port);
		smtpUser = c.smtp_user;
		smtpPassword = '';
		passwordReplacing = false;
		passwordSaved = savedSecrets.includes('smtp_password');
		smtpEncryption = c.smtp_encryption;
		toAddrs = c.to_addrs.join(', ');
	}

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});
</script>

<PageTitle title="Email · Integrations" />

<div class="space-y-8">
	<div class="flex h-14 items-center gap-4">
		<a
			href="/integrations"
			class="text-muted-foreground hover:text-foreground"
			aria-label="Back to integrations"
		>
			<ChevronLeft class="h-5 w-5" />
		</a>
		<div class="flex items-center gap-3">
			<Mail class="h-6 w-6" />
			<h1 class="text-2xl font-extrabold sm:text-3xl">Email (SMTP)</h1>
		</div>
	</div>

	<NotificationChannelList
		kind="email"
		label="Email"
		initialIntegrations={integrations}
		{buildConfig}
		{resetConfig}
		{loadConfig}
	>
		{#snippet configFields()}
			<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
				<div class="grid gap-1.5">
					<Label class="text-xs" for="em-from-name">From Name</Label>
					<Input
						id="em-from-name"
						class="h-8 text-xs"
						bind:value={fromName}
						placeholder="Tond Alerts"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="em-from-addr">From Address</Label>
					<Input
						id="em-from-addr"
						class="h-8 text-xs"
						bind:value={fromAddr}
						placeholder="alerts@example.com"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="em-host">SMTP Host</Label>
					<Input
						id="em-host"
						class="h-8 text-xs"
						bind:value={smtpHost}
						placeholder="smtp.example.com"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="em-port">SMTP Port</Label>
					<Input id="em-port" class="h-8 text-xs" bind:value={smtpPort} placeholder="587" />
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="em-user">SMTP Username</Label>
					<Input
						id="em-user"
						class="h-8 text-xs"
						bind:value={smtpUser}
						placeholder="alerts@example.com"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="em-pass">SMTP Password</Label>
					<SavedSecretField
						id="em-pass"
						saved={passwordSaved}
						bind:value={smtpPassword}
						bind:replacing={passwordReplacing}
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="em-enc">Encryption</Label>
					<select
						id="em-enc"
						bind:value={smtpEncryption}
						class="h-8 border bg-background px-2 text-xs"
					>
						<option value="starttls">STARTTLS (port 587)</option>
						<option value="tls">TLS (port 465)</option>
						<option value="none">None (not recommended)</option>
					</select>
				</div>
				<div class="grid gap-1.5 sm:col-span-2">
					<Label class="text-xs" for="em-to">Recipient Addresses</Label>
					<textarea
						id="em-to"
						bind:value={toAddrs}
						class="h-16 border bg-background p-2 font-mono text-xs"
						placeholder="one@example.com, two@example.com"></textarea>
					<p class="text-[10px] text-muted-foreground">
						Separate multiple addresses with commas or newlines.
					</p>
				</div>
			</div>
		{/snippet}
	</NotificationChannelList>
</div>
