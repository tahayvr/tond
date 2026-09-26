<script lang="ts">
	import type { Project, Template, TemplateDeployResult, TemplateHostMemory } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { categoryLabel, formatRam } from '$lib/templates';
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import TemplateLogo from '$lib/components/templates/template-logo.svelte';
	import MemoryBadge from '$lib/components/templates/memory-badge.svelte';
	import TemplateDeployForm from '$lib/components/templates/template-deploy-form.svelte';
	import TemplateCredentials from '$lib/components/templates/template-credentials.svelte';
	import GlobeIcon from '~icons/ri/global-line';
	import BookIcon from '~icons/ri/book-open-line';
	import CodeIcon from '~icons/ri/code-s-slash-line';
	import ServerIcon from '~icons/ri/server-line';
	import HardDriveIcon from '~icons/ri/hard-drive-2-line';
	import ArrowUpIcon from '~icons/ri/arrow-up-circle-line';
	import ExternalIcon from '~icons/ri/external-link-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let template = $derived(data.template as Template);
	let memory = $derived(data.memory as TemplateHostMemory | null);
	let projects = $derived(data.projects as Project[]);
	let result = $state<TemplateDeployResult | null>(null);
	let showCompose = $state(false);

	let links = $derived(
		[
			{ href: template.website, label: 'Website', icon: GlobeIcon },
			{ href: template.docs, label: 'Documentation', icon: BookIcon },
			{ href: template.repository, label: 'Source code', icon: CodeIcon }
		].filter((l): l is { href: string; label: string; icon: typeof GlobeIcon } => !!l.href)
	);

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<PageTitle title={template.name} section="Templates" />

<div class="space-y-6 pb-12">
	<Breadcrumb items={[{ label: 'Templates', href: '/templates' }, { label: template.name }]} />

	<div class="flex items-start gap-4">
		<TemplateLogo {template} size="lg" />
		<div class="min-w-0 space-y-1.5">
			<h1 class="text-2xl font-extrabold break-words sm:text-3xl">{template.name}</h1>
			<p class="text-sm text-muted-foreground">{template.description}</p>
			<div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-muted-foreground">
				<a
					href="/templates?category={template.category}"
					class="hover:text-foreground hover:underline">{categoryLabel(template.category)}</a
				>
				<span>Version {template.version}</span>
				<MemoryBadge {template} {memory} />
			</div>
		</div>
	</div>

	<div class="grid gap-6 lg:grid-cols-[minmax(0,1fr)_380px] lg:items-start">
		<div class="space-y-6">
			{#if template.about}
				<section class="space-y-2" aria-labelledby="about-heading">
					<h2 id="about-heading" class="text-sm font-semibold">About</h2>
					<p class="text-sm leading-relaxed text-muted-foreground">{template.about}</p>
					{#if links.length > 0}
						<ul class="flex flex-wrap gap-x-4 gap-y-1 pt-1 text-sm">
							{#each links as link (link.label)}
								<li>
									<a
										href={link.href}
										target="_blank"
										rel="noopener noreferrer"
										class="inline-flex items-center gap-1.5 text-foreground hover:underline"
									>
										<link.icon class="size-4 text-muted-foreground" aria-hidden="true" />
										{link.label}
										<ExternalIcon class="size-3 text-muted-foreground" aria-hidden="true" />
										<span class="sr-only">(opens in a new tab)</span>
									</a>
								</li>
							{/each}
						</ul>
					{/if}
				</section>
			{/if}

			<section class="space-y-2" aria-labelledby="services-heading">
				<h2 id="services-heading" class="flex items-center gap-2 text-sm font-semibold">
					<ServerIcon class="size-4 text-muted-foreground" aria-hidden="true" />
					Services
				</h2>
				<ul class="divide-y rounded-lg border bg-card text-sm">
					{#each template.services as service (service.name)}
						<li class="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2.5">
							<span class="font-medium">{service.name}</span>
							{#if service.public}
								<span
									class="rounded-sm border border-primary/40 bg-primary/10 px-1.5 py-0.5 text-xs text-primary"
									title="Gets the app's domain and HTTPS">Public · port {template.public.port}</span
								>
							{:else}
								<span class="text-xs text-muted-foreground">Private to the app</span>
							{/if}
							<span
								class="min-w-0 flex-1 truncate text-right font-mono text-xs text-muted-foreground sm:flex-none sm:text-left"
								>{service.image}</span
							>
							<span class="ml-auto text-xs text-muted-foreground tabular-nums"
								>limit {formatRam(service.memory_limit_mb)}</span
							>
						</li>
					{/each}
				</ul>
			</section>

			{#if template.volumes.length > 0}
				<section class="space-y-2" aria-labelledby="volumes-heading">
					<h2 id="volumes-heading" class="flex items-center gap-2 text-sm font-semibold">
						<HardDriveIcon class="size-4 text-muted-foreground" aria-hidden="true" />
						Stored data
					</h2>
					<ul class="divide-y rounded-lg border bg-card text-sm">
						{#each template.volumes as volume (volume.service + volume.path)}
							<li class="space-y-0.5 px-3 py-2.5">
								<p>
									<span class="font-mono text-xs">{volume.path}</span>
									<span class="text-xs text-muted-foreground">· {volume.service}</span>
								</p>
								<p class="text-xs text-muted-foreground">{volume.description}</p>
							</li>
						{/each}
					</ul>
					<p class="text-xs text-muted-foreground">
						Kept in the app's volume directory, so snapshots include it.
					</p>
				</section>
			{/if}

			{#if template.setup_notes || template.post_deploy_checks.length > 0}
				<section class="space-y-2" aria-labelledby="setup-heading">
					<h2 id="setup-heading" class="text-sm font-semibold">After deploying</h2>
					{#if template.setup_notes}
						<p class="text-sm text-muted-foreground">{template.setup_notes}</p>
					{/if}
					{#if template.post_deploy_checks.length > 0}
						<ul class="list-disc space-y-0.5 pl-5 text-sm text-muted-foreground">
							{#each template.post_deploy_checks as check (check)}
								<li>{check}</li>
							{/each}
						</ul>
					{/if}
				</section>
			{/if}

			{#if template.upgrade_notes}
				<section id="upgrading" class="scroll-mt-20 space-y-2" aria-labelledby="upgrade-heading">
					<h2 id="upgrade-heading" class="flex items-center gap-2 text-sm font-semibold">
						<ArrowUpIcon class="size-4 text-muted-foreground" aria-hidden="true" />
						Upgrading
					</h2>
					<p class="text-sm text-muted-foreground">{template.upgrade_notes}</p>
				</section>
			{/if}

			<section class="space-y-2">
				<button
					type="button"
					class="text-sm font-medium underline-offset-4 hover:underline"
					aria-expanded={showCompose}
					aria-controls="compose-file"
					onclick={() => (showCompose = !showCompose)}
				>
					{showCompose ? 'Hide' : 'Show'} the Compose file
				</button>
				{#if showCompose}
					<pre
						id="compose-file"
						class="max-h-[32rem] overflow-auto rounded-lg border bg-muted/40 p-3 font-mono text-xs leading-relaxed">{template.compose}</pre>
					<p class="text-xs text-muted-foreground">
						<span class="font-mono">${'{…}'}</span> values come from the app's environment variables,
						where Tond stores the inputs encrypted. You can edit both after the deploy.
					</p>
				{/if}
			</section>
		</div>

		<aside
			class="order-first space-y-4 rounded-lg border bg-card p-4 lg:sticky lg:top-20 lg:order-none"
			aria-labelledby="deploy-heading"
		>
			<h2 id="deploy-heading" class="font-semibold">
				{result ? 'Deploy started' : `Deploy ${template.name}`}
			</h2>
			{#if result}
				<TemplateCredentials {result} templateName={template.name} />
			{:else}
				{#key template.id}
					<TemplateDeployForm
						{template}
						{memory}
						appsDomain={data.appsDomain as string}
						serverArch={data.architecture as string}
						{projects}
						ondeployed={(r) => (result = r)}
					/>
				{/key}
			{/if}
		</aside>
	</div>
</div>
