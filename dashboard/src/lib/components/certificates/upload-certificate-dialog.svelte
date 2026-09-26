<!--
  UploadCertificateDialog — serve your own certificate for a domain (or a
  wildcard). tond-proxy checks it before serving: the key matches the
  certificate, the chain is in order, it is valid now and covers the domain.
  Uploaded certificates are never renewed by Tond; expiry alerts still apply.
-->
<script lang="ts">
	import { uploadCertificate, type Certificate } from '$lib/api';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { normalizeDomainInput } from '$lib/domains';

	let {
		open = $bindable(false),
		domain: initialDomain = '',
		onUploaded
	}: {
		open?: boolean;
		/** Prefilled domain (Replace on an existing row). */
		domain?: string;
		onUploaded?: (cert: Certificate | null, domain: string) => unknown;
	} = $props();

	let domain = $state('');
	let certPem = $state('');
	let keyPem = $state('');
	let saving = $state(false);
	let error = $state('');

	$effect(() => {
		if (open) {
			domain = initialDomain;
			certPem = '';
			keyPem = '';
			error = '';
		}
	});

	async function readFile(e: Event, set: (text: string) => void) {
		const file = (e.currentTarget as HTMLInputElement).files?.[0];
		if (!file) return;
		if (file.size > 64 * 1024) {
			error = `${file.name} is larger than 64 KiB; that isn't a PEM certificate or key.`;
			return;
		}
		set(await file.text());
	}

	async function handleSubmit(e: Event) {
		e.preventDefault();
		const name = normalizeDomainInput(domain);
		if (!name) return void (error = 'Enter the domain the certificate is for.');
		if (!certPem.includes('-----BEGIN CERTIFICATE-----')) {
			return void (error =
				'Paste the certificate chain as PEM (-----BEGIN CERTIFICATE-----), the certificate first, then the intermediates.');
		}
		if (!keyPem.includes('PRIVATE KEY-----')) {
			return void (error = 'Paste the private key as PEM (-----BEGIN PRIVATE KEY-----).');
		}
		saving = true;
		error = '';
		try {
			const cert = await uploadCertificate({ domain: name, cert_pem: certPem, key_pem: keyPem });
			keyPem = '';
			open = false;
			await onUploaded?.(cert, name);
		} catch (err: any) {
			error = err.message ?? 'The upload failed';
		} finally {
			saving = false;
		}
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="max-h-[90vh] overflow-y-auto sm:max-w-xl">
		<Dialog.Header>
			<Dialog.Title>Upload a certificate</Dialog.Title>
			<Dialog.Description>
				Serve a certificate from your own CA instead of Let's Encrypt. Tond checks it before serving
				it and never renews it: upload the renewed one before it expires (expiry alerts remind you).
			</Dialog.Description>
		</Dialog.Header>
		<form class="space-y-4 py-2" onsubmit={handleSubmit}>
			<div class="grid gap-1.5">
				<Label for="upload-domain">Domain</Label>
				<Input
					id="upload-domain"
					bind:value={domain}
					placeholder="app.example.com or *.example.com"
					autocomplete="off"
					spellcheck={false}
					disabled={saving}
				/>
			</div>
			<div class="grid gap-1.5">
				<div class="flex items-center justify-between gap-2">
					<Label for="upload-cert">Certificate chain (PEM)</Label>
					<label
						class="cursor-pointer text-xs text-muted-foreground underline-offset-2 hover:underline"
					>
						Load file
						<input
							type="file"
							class="sr-only"
							accept=".pem,.crt,.cer,text/plain"
							onchange={(e) => readFile(e, (t) => (certPem = t))}
						/>
					</label>
				</div>
				<Textarea
					id="upload-cert"
					bind:value={certPem}
					rows={6}
					class="font-mono text-xs"
					placeholder="-----BEGIN CERTIFICATE-----"
					spellcheck={false}
					disabled={saving}
				/>
				<p class="text-xs text-muted-foreground">Your certificate first, then each intermediate.</p>
			</div>
			<div class="grid gap-1.5">
				<div class="flex items-center justify-between gap-2">
					<Label for="upload-key">Private key (PEM)</Label>
					<label
						class="cursor-pointer text-xs text-muted-foreground underline-offset-2 hover:underline"
					>
						Load file
						<input
							type="file"
							class="sr-only"
							accept=".pem,.key,text/plain"
							onchange={(e) => readFile(e, (t) => (keyPem = t))}
						/>
					</label>
				</div>
				<Textarea
					id="upload-key"
					bind:value={keyPem}
					rows={4}
					class="font-mono text-xs"
					placeholder="-----BEGIN PRIVATE KEY-----"
					spellcheck={false}
					autocomplete="off"
					disabled={saving}
				/>
				<p class="text-xs text-muted-foreground">
					Unencrypted (no passphrase). Only tond-proxy can read it once stored.
				</p>
			</div>
			<InlineError message={error} ondismiss={() => (error = '')} />
			<Dialog.Footer>
				<Button type="button" variant="ghost" onclick={() => (open = false)} disabled={saving}
					>Cancel</Button
				>
				<Button type="submit" disabled={saving}
					>{saving ? 'Checking…' : 'Upload certificate'}</Button
				>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
