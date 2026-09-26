<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import CertificateStatus from '$lib/components/domains/certificate-status.svelte';

	interface Props {
		domainInput: string;
		/** The domain currently saved on the app (drives the certificate status). */
		savedDomain?: string | null;
		/** Bumped after a save so the certificate status re-fetches. */
		certRefreshKey?: number;
		onUpdate: () => void;
	}

	let {
		domainInput = $bindable(''),
		savedDomain = null,
		certRefreshKey = 0,
		onUpdate
	}: Props = $props();
</script>

<div class="space-y-4 pt-4">
	<p class="text-sm text-muted-foreground">
		Set a custom domain for this app. Point its A (and AAAA, if you use IPv6) record at this server;
		Tond then requests a Let's Encrypt certificate automatically.
	</p>
	<div class="flex gap-3">
		<div class="grid flex-1 gap-2">
			<Label for="domain-input">Domain</Label>
			<Input id="domain-input" type="text" bind:value={domainInput} placeholder="app.example.com" />
		</div>
		<Button onclick={onUpdate} class="self-end">Update Domain</Button>
	</div>
	<CertificateStatus domain={savedDomain} refreshKey={certRefreshKey} />
</div>
