<script lang="ts">
	import { page } from '$app/state';
	import {
		getApp,
		deployApp,
		stopApp,
		startApp,
		restartApp,
		deleteApp,
		updateDomain,
		updateHealth,
		updateDrainTimeout,
		updateAppResources,
		addVolume,
		removeVolume,
		listDeployments,
		getDeploymentGeneratedDockerfile,
		rollbackApp,
		generateDeployKey,
		getDeployKey,
		setAutoDeploy,
		updateSource,
		listPreviews,
		getSettings,
		createPreview,
		deletePreview,
		updatePreviewConfig,
		listOpenPRs,
		listRegistryCredentials,
		createRegistryCredential,
		deleteRegistryCredential,
		setAppRegistryCredential,
		listGitHubApps,
		listGitHubRepos,
		listGitHubBranches,
		setAppGitHubApp,
		type App,
		type Deployment,
		type DeployKey,
		type PreviewDeployment,
		type GithubPR,
		type RegistryCredential,
		type GitHubApp,
		type GitHubRepo,
		type GitHubBranch,
		type LogLine,
		openLogStream
	} from '$lib/api';
	import { goto } from '$app/navigation';
	import { can, isAdmin } from '$lib/auth.svelte';
	import { setUiContext, clearUiContext, appContext, openFeedPanel } from '$lib/ui-context.svelte';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import ConfirmDeleteDialog from '$lib/components/dialogs/confirm-delete-dialog.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import AppOverview from '$lib/components/tabs/app-page/app-overview.svelte';
	import AppLogs from '$lib/components/tabs/app-page/app-logs.svelte';
	import AppDeployments from '$lib/components/tabs/app-page/app-deployments.svelte';
	import AppEnvironment from '$lib/components/tabs/app-page/app-environment.svelte';
	import AppDomain from '$lib/components/tabs/app-page/app-domain.svelte';
	import AppDatabases from '$lib/components/tabs/app-page/app-databases.svelte';
	import AppHealth from '$lib/components/tabs/app-page/app-health.svelte';
	import AppPort from '$lib/components/tabs/app-page/app-port.svelte';
	import AppVolumes from '$lib/components/tabs/app-page/app-volumes.svelte';
	import AppPreviews from '$lib/components/tabs/app-page/app-previews.svelte';
	import PreviewDomainRequiredAlert from '$lib/components/alerts/preview-domain-required-alert.svelte';
	import MetricsPanel from '$lib/components/metrics/metrics-panel.svelte';
	import AppHistory from '$lib/components/tabs/app-page/app-history.svelte';
	import AppWebhooks from '$lib/components/tabs/app-page/app-webhooks.svelte';
	import { safeUrl } from '$lib/utils';
	import { SecretInput } from '$lib/components/ui/secret-input/index.js';
	import Check from '~icons/ri/check-line';
	import Github from '~icons/ri/github-fill';
	import GitBranch from '~icons/ri/git-branch-line';
	import Unlink2 from '~icons/ri/link-unlink-m';
	import Lock from '~icons/ri/lock-line';
	import SpinnerIcon from '~icons/ri/loader-4-line';
	import type { PageProps } from './$types';

	import { createLogBuffer } from '$lib/log-buffer';
	import { pollWhileVisible } from '$lib/polling';
	import { untrack } from 'svelte';

	let { data, params }: PageProps = $props();

	const APP_TABS = [
		'overview',
		'logs',
		'deployments',
		'env',
		'domain',
		'health',
		'volumes',
		'resources',
		'git',
		'previews',
		'metrics',
		'history'
	];
	/** The running deployment while the app builds (for the "View deploy" link). */
	let activeDeploymentId = $state<string | null>(null);

	let app = $derived(data.app as App | null);
	let error = $state('');
	let showDeleteDialog = $state(false);

	// Status polling runs while the app is building (or right after a deploy
	// request) and stops on a terminal status. See the effects at the bottom.
	let statusPollRequested = $state(false);
	let appStatus = $derived(app?.status);
	/** Selected tab; drives the log stream, metrics polling and feed context. */
	let activeTab = $state('overview');

	// Domain form
	let certRefreshKey = $state(0);

	// Health form
	let healthPath = $state('');
	let healthInterval = $state(30);
	let healthThreshold = $state(3);
	let drainTimeout = $state(30);

	// Volume form
	let volumeContainerPath = $state('');
	let volumeName = $state('');
	let volumeError = $state('');

	// Resources form
	let resourceMemoryDraft = $state<number | null>(null);
	let resourceMemoryUnit = $state<'mb' | 'gb'>('mb');
	let resourceCpuDraft = $state<number | null>(null);
	let savingResources = $state(false);
	let resourcesError = $state('');
	let resourcesSaved = $state(false);
	let resourcesSavedTimeout: ReturnType<typeof setTimeout> | null = null;

	// Deployments state
	let deployments = $state<Deployment[]>([]);
	let deploymentsLoading = $state(false);
	let rollbackError = $state('');

	// Git integration state
	let deployKey = $state<DeployKey | null>(null);
	let deployKeyLoading = $state(false);
	let deployKeyError = $state('');
	let gitError = $state('');
	let gitSuccess = $state('');
	let watchBranchInput = $state('');
	let sourceTypeInput = $state<'git' | 'image' | 'tarball'>('git');
	let sourceUrlInput = $state('');

	// Registry credentials state
	let registryCredentials = $state<RegistryCredential[]>([]);
	let registryCredentialsLoading = $state(false);
	let selectedCredentialId = $state<string | null>(null);
	let newCredName = $state('');
	let newCredServer = $state('');
	let newCredUsername = $state('');
	let newCredPassword = $state('');
	let addCredOpen = $state(false);
	let addCredError = $state('');
	let credError = $state('');

	// GitHub App source picker state
	let githubApps = $state<GitHubApp[]>([]);
	let githubPickerRepos = $state<Record<string, GitHubRepo[]>>({});
	let githubPickerBranches = $state<Record<string, GitHubBranch[]>>({});
	let selectedGithubAppId = $state<string | null>(null);
	let selectedGithubRepo = $state<GitHubRepo | null>(null);
	let selectedGithubBranch = $state<string | null>(null);
	let githubPickerReposLoading = $state(false);
	let githubPickerBranchesLoading = $state(false);
	let githubLinkError = $state('');
	let githubLinkSaving = $state(false);

	// Preview deployments state
	let previews = $state<PreviewDeployment[]>([]);
	let previewsLoading = $state(false);
	let previewsError = $state('');
	let previewTtl = $state(24);
	let previewConfigSaving = $state(false);
	let previewConfigError = $state('');
	// null until settings are loaded; '' when no preview domain is configured
	let previewDomain = $state<string | null>(null);
	let previewsBlocked = $derived(previewDomain === '');

	// Open PRs state (Load open PRs feature)
	let openPRs = $state<GithubPR[]>([]);
	let openPRsLoading = $state(false);
	let openPRsError = $state('');
	let deployingPR = $state<number | null>(null);

	// Logs state — bounded (last LOG_BUFFER_LIMIT lines) and batched per flush.
	let logLines = $state.raw<LogLine[]>([]);
	let logStreaming = $state(false);
	let closeLogSocket: (() => void) | null = null;
	let deploying = $state(false);
	const logBuffer = createLogBuffer<LogLine>((lines) => (logLines = lines));

	let appId = $derived(page.params.id || params.id);

	/** Reset every settings form on the page to the app's saved values. */
	function initForms(a: App) {
		healthPath = a.health_check_path || '';
		healthInterval = a.health_check_interval;
		healthThreshold = a.health_failure_threshold;
		drainTimeout = a.drain_timeout;
		// Git integration init
		watchBranchInput = a.watch_branch || '';
		sourceTypeInput = (a.source_type as 'git' | 'image' | 'tarball') || 'git';
		sourceUrlInput = a.source_url || '';
		selectedCredentialId = a.registry_credential_id;
		initResourcesDraft();
	}

	// Fill the forms when the page's app is loaded or changes (the loader
	// provides it; without this the forms start empty and a save would
	// overwrite the real settings with blanks).
	let formsInitializedFor: string | null = null;
	$effect(() => {
		const current = app;
		if (!current || current.id === formsInitializedFor) return;
		formsInitializedFor = current.id;
		untrack(() => initForms(current));
	});

	async function loadApp() {
		try {
			app = await getApp(appId);
			initForms(app);
		} catch (e: any) {
			error = e.message;
		}
	}

	async function pollStatus() {
		try {
			const latest = await getApp(appId);
			if (latest.status !== 'building') {
				// Terminal state: reload everything once; the polling effect stops.
				statusPollRequested = false;
				await loadApp();
			} else if (app && app.status !== latest.status) {
				// Only update the status without resetting form inputs.
				app = { ...app, status: latest.status };
			}
		} catch {
			// Non-critical — keep polling
		}
	}

	function stopLogStream() {
		closeLogSocket?.();
		closeLogSocket = null;
		logBuffer.flush();
		logStreaming = false;
	}

	function startLogStream() {
		stopLogStream();
		logBuffer.clear();
		logStreaming = true;
		const close = openLogStream(`/api/apps/${appId}/logs/stream`, {
			onLine(line) {
				logBuffer.push(line);
			},
			// Reconnects with backoff if the server restarts; a fresh snapshot follows.
			onOpen(reconnected) {
				logStreaming = true;
				if (reconnected) logBuffer.clear();
			},
			onDisconnect() {
				logBuffer.flush();
				logStreaming = false;
			}
		});
		closeLogSocket = close;
	}

	/** Start a deploy and open its live view. */
	async function startDeploy() {
		const started = await deployApp(appId);
		await goto(`/apps/${appId}/deployments/${started.deployment_id}`);
	}

	async function handleDeploy() {
		deploying = true;
		error = '';
		try {
			await startDeploy();
		} catch (e: any) {
			error = e.message;
		} finally {
			deploying = false;
		}
	}

	async function handleStop() {
		try {
			openFeedPanel();
			await stopApp(appId);
			await loadApp();
		} catch (e: any) {
			error = e.message;
		}
	}

	async function handleStart() {
		try {
			openFeedPanel();
			await startApp(appId);
			await loadApp();
		} catch (e: any) {
			error = e.message;
		}
	}

	async function handleRestart() {
		try {
			openFeedPanel();
			await restartApp(appId);
			await loadApp();
		} catch (e: any) {
			error = e.message;
		}
	}

	async function handleDelete() {
		try {
			await deleteApp(appId);
			goto('/');
		} catch (e: any) {
			error = e.message;
		}
	}

	// Environment tab: errors are shown inside the tab, so these rethrow.
	async function handleEnvRegistryCredential(credentialId: string | null) {
		await setAppRegistryCredential(appId, credentialId);
		await loadApp();
	}

	async function handleEnvRedeploy() {
		await startDeploy();
	}

	/** Domain card save; errors are shown in the card, so this rethrows. */
	async function handleUpdateDomain(domain: string) {
		await updateDomain(appId, domain);
		await loadApp();
		certRefreshKey += 1;
	}

	async function handleUpdateHealth() {
		try {
			await updateHealth(appId, healthPath.trim() || null, healthInterval, healthThreshold);
			await loadApp();
			error = '';
		} catch (e: any) {
			error = e.message;
		}
	}

	async function handleDisableHealth() {
		try {
			await updateHealth(appId, null, 30, 3);
			await loadApp();
			error = '';
		} catch (e: any) {
			error = e.message;
		}
	}

	async function handleUpdateDrainTimeout() {
		try {
			await updateDrainTimeout(appId, drainTimeout);
			await loadApp();
			error = '';
		} catch (e: any) {
			error = e.message;
		}
	}

	async function handleAddVolume() {
		volumeError = '';
		const path = volumeContainerPath.trim();
		const name = volumeName.trim();
		if (!path || !path.startsWith('/')) {
			volumeError = 'Container path must be an absolute path (e.g. /data)';
			return;
		}
		try {
			await addVolume(appId, { container_path: path, ...(name ? { name } : {}) });
			volumeContainerPath = '';
			volumeName = '';
			await loadApp();
		} catch (e: any) {
			volumeError = e.message;
		}
	}

	async function handleRemoveVolume(containerPath: string) {
		volumeError = '';
		try {
			await removeVolume(appId, containerPath);
			await loadApp();
		} catch (e: any) {
			volumeError = e.message;
		}
	}

	async function loadDeployments() {
		deploymentsLoading = true;
		try {
			deployments = await listDeployments(appId);
		} finally {
			deploymentsLoading = false;
		}
	}

	async function handleRollback(deploymentId: string) {
		rollbackError = '';
		try {
			const started = await rollbackApp(appId, deploymentId);
			await goto(`/apps/${appId}/deployments/${started.deployment_id}`);
		} catch (e: any) {
			rollbackError = e.message;
		}
	}

	async function handleViewGeneratedDockerfile(deploymentId: string): Promise<string> {
		return getDeploymentGeneratedDockerfile(appId, deploymentId);
	}

	async function loadDeployKey() {
		deployKeyLoading = true;
		deployKeyError = '';
		try {
			deployKey = await getDeployKey(appId);
		} catch {
			deployKey = null;
		} finally {
			deployKeyLoading = false;
		}
	}

	async function handleGenerateDeployKey() {
		deployKeyError = '';
		deployKeyLoading = true;
		try {
			deployKey = await generateDeployKey(appId);
		} catch (e: any) {
			deployKeyError = e.message;
		} finally {
			deployKeyLoading = false;
		}
	}

	async function handleSetAutoDeploy(enabled: boolean) {
		gitError = '';
		gitSuccess = '';
		try {
			await setAutoDeploy(appId, enabled, watchBranchInput.trim() || null);
			await loadApp();
			gitSuccess = enabled ? 'Auto-deploy enabled.' : 'Auto-deploy disabled.';
		} catch (e: any) {
			gitError = e.message;
		}
	}

	async function handleSaveWatchBranch() {
		gitError = '';
		gitSuccess = '';
		try {
			const currentAutoDeploy = app?.auto_deploy ?? false;
			await setAutoDeploy(appId, currentAutoDeploy, watchBranchInput.trim() || null);
			await loadApp();
			gitSuccess = 'Watch branch updated.';
		} catch (e: any) {
			gitError = e.message;
		}
	}

	async function handleUpdateSource() {
		gitError = '';
		gitSuccess = '';
		try {
			await updateSource(appId, sourceTypeInput, sourceUrlInput.trim() || null);
			await loadApp();
			gitSuccess = 'Source updated.';
		} catch (e: any) {
			gitError = e.message;
		}
	}

	async function loadRegistryCredentials() {
		registryCredentialsLoading = true;
		try {
			registryCredentials = await listRegistryCredentials();
		} catch {
			// non-critical — silently ignore
		} finally {
			registryCredentialsLoading = false;
		}
	}

	async function loadGitHubApps() {
		try {
			githubApps = await listGitHubApps();
		} catch {
			// non-critical — silently ignore
		}
	}

	async function handleGithubAppChange(appId: string | null) {
		selectedGithubAppId = appId;
		selectedGithubRepo = null;
		selectedGithubBranch = null;
		if (!appId) return;
		// Load repos for the selected app if not already cached
		if (!githubPickerRepos[appId]) {
			githubPickerReposLoading = true;
			try {
				githubPickerRepos[appId] = await listGitHubRepos(appId);
			} catch {
				githubPickerRepos[appId] = [];
			} finally {
				githubPickerReposLoading = false;
			}
		}
	}

	async function handleGithubRepoChange(repo: GitHubRepo | null) {
		selectedGithubRepo = repo;
		selectedGithubBranch = null;
		if (!selectedGithubAppId || !repo) return;
		const key = `${selectedGithubAppId}/${repo.full_name}`;
		if (!githubPickerBranches[key]) {
			githubPickerBranchesLoading = true;
			const [owner, repoName] = repo.full_name.split('/');
			try {
				githubPickerBranches[key] = await listGitHubBranches(selectedGithubAppId, owner, repoName);
			} catch {
				githubPickerBranches[key] = [];
			} finally {
				githubPickerBranchesLoading = false;
			}
		}
		// Pre-select the default branch
		selectedGithubBranch = repo.default_branch;
	}

	async function handleGithubLink() {
		if (!selectedGithubAppId || !selectedGithubRepo) return;
		githubLinkSaving = true;
		githubLinkError = '';
		gitSuccess = '';
		try {
			// Set the clone URL in the source field
			sourceUrlInput = selectedGithubRepo.clone_url;
			// Save the source URL and github_app_id together
			await updateSource(appId, 'git', selectedGithubRepo.clone_url);
			await setAppGitHubApp(appId, selectedGithubAppId);
			// Also save the watch branch if one is selected
			if (selectedGithubBranch) {
				watchBranchInput = selectedGithubBranch;
				await setAutoDeploy(appId, app?.auto_deploy ?? false, selectedGithubBranch);
			}
			await loadApp();
			gitSuccess = `Linked to ${selectedGithubRepo.full_name}`;
		} catch (e: any) {
			githubLinkError = e.message ?? 'Failed to link GitHub App';
		} finally {
			githubLinkSaving = false;
		}
	}

	async function handleGithubUnlink() {
		githubLinkSaving = true;
		githubLinkError = '';
		gitSuccess = '';
		try {
			await setAppGitHubApp(appId, null);
			await loadApp();
			selectedGithubAppId = null;
			selectedGithubRepo = null;
			selectedGithubBranch = null;
			gitSuccess = 'GitHub App unlinked.';
		} catch (e: any) {
			githubLinkError = e.message ?? 'Failed to unlink GitHub App';
		} finally {
			githubLinkSaving = false;
		}
	}

	async function handleSetRegistryCredential(credId: string | null) {
		credError = '';
		gitSuccess = '';
		try {
			await setAppRegistryCredential(appId, credId);
			await loadApp();
			selectedCredentialId = credId;
			gitSuccess = credId ? 'Registry credential attached.' : 'Registry credential cleared.';
		} catch (e: any) {
			credError = e.message;
		}
	}

	async function handleAddCredential() {
		addCredError = '';
		if (
			!newCredName.trim() ||
			!newCredServer.trim() ||
			!newCredUsername.trim() ||
			!newCredPassword.trim()
		) {
			addCredError = 'All fields are required.';
			return;
		}
		try {
			await createRegistryCredential({
				name: newCredName.trim(),
				server: newCredServer.trim(),
				username: newCredUsername.trim(),
				password: newCredPassword
			});
			newCredName = '';
			newCredServer = '';
			newCredUsername = '';
			newCredPassword = '';
			addCredOpen = false;
			await loadRegistryCredentials();
		} catch (e: any) {
			addCredError = e.message;
		}
	}

	async function handleDeleteCredential(credId: string) {
		credError = '';
		try {
			await deleteRegistryCredential(credId);
			// If the deleted credential was attached to this app, clear it
			if (app?.registry_credential_id === credId) {
				await setAppRegistryCredential(appId, null);
				await loadApp();
			}
			await loadRegistryCredentials();
		} catch (e: any) {
			credError = e.message;
		}
	}

	// ── Resources ──

	function initResourcesDraft() {
		if (!app) return;
		if (app.memory_limit === null) {
			resourceMemoryDraft = null;
			resourceMemoryUnit = 'mb';
		} else {
			const mb = app.memory_limit / (1024 * 1024);
			if (mb >= 1024 && mb % 1024 === 0) {
				resourceMemoryDraft = mb / 1024;
				resourceMemoryUnit = 'gb';
			} else {
				resourceMemoryDraft = mb;
				resourceMemoryUnit = 'mb';
			}
		}
		if (app.cpu_limit === null) {
			resourceCpuDraft = null;
		} else {
			resourceCpuDraft = app.cpu_limit / 1_000_000_000;
		}
	}

	async function handleSaveResources() {
		savingResources = true;
		resourcesError = '';
		try {
			let memoryLimitBytes: number | null = null;
			if (resourceMemoryDraft !== null && resourceMemoryDraft > 0) {
				memoryLimitBytes =
					resourceMemoryUnit === 'gb'
						? resourceMemoryDraft * 1024 * 1024 * 1024
						: resourceMemoryDraft * 1024 * 1024;
			}
			let cpuLimitNano: number | null = null;
			if (resourceCpuDraft !== null && resourceCpuDraft > 0) {
				cpuLimitNano = Math.round(resourceCpuDraft * 1_000_000_000);
			}
			app = await updateAppResources(appId, memoryLimitBytes, cpuLimitNano);
			if (app) setUiContext(appContext(app));
			resourcesSaved = true;
			if (resourcesSavedTimeout) clearTimeout(resourcesSavedTimeout);
			resourcesSavedTimeout = setTimeout(() => (resourcesSaved = false), 2000);
		} catch (e: any) {
			resourcesError = e.message ?? 'Failed to save resources';
		} finally {
			savingResources = false;
		}
	}

	async function loadPreviews() {
		previewsLoading = true;
		previewsError = '';
		try {
			previews = await listPreviews(appId);
		} catch (e: any) {
			previewsError = e.message;
		} finally {
			previewsLoading = false;
		}
		try {
			previewDomain = (await getSettings()).preview_domain ?? '';
		} catch {
			// Leave the state unknown; the API still refuses previews without a domain.
		}
	}

	async function handleCreatePreview(options: Parameters<typeof createPreview>[1] = {}) {
		previewsError = '';
		try {
			await createPreview(appId, { ttl_hours: previewTtl, ...options });
			await loadPreviews();
		} catch (e: any) {
			previewsError = e.message;
		}
	}

	async function handleLoadOpenPRs() {
		openPRsLoading = true;
		openPRsError = '';
		try {
			openPRs = await listOpenPRs(appId);
		} catch (e: any) {
			openPRsError = e.message;
		} finally {
			openPRsLoading = false;
		}
	}

	async function handleDeployPR(pr: GithubPR) {
		deployingPR = pr.number;
		previewsError = '';
		try {
			await createPreview(appId, {
				ttl_hours: previewTtl,
				pr_number: pr.number,
				head_sha: pr.head_sha,
				head_ref: pr.head_ref,
				pr_url: pr.pr_url
			});
			await loadPreviews();
		} catch (e: any) {
			previewsError = e.message;
		} finally {
			deployingPR = null;
		}
	}

	async function handleDeletePreview(previewId: string) {
		previewsError = '';
		try {
			await deletePreview(appId, previewId);
			await loadPreviews();
		} catch (e: any) {
			previewsError = e.message;
		}
	}

	async function handleTogglePreviewEnabled(enabled: boolean) {
		previewConfigSaving = true;
		previewConfigError = '';
		try {
			await updatePreviewConfig(appId, enabled);
			await loadApp();
		} catch (e: any) {
			previewConfigError = e.message;
		} finally {
			previewConfigSaving = false;
		}
	}

	/** Load what a tab needs when it opens. */
	function handleTabChange(v: string) {
		if (v === 'deployments') {
			loadDeployments();
		}
		if (v === 'env') {
			loadRegistryCredentials();
		}
		if (v === 'git') {
			loadDeployKey();
			loadRegistryCredentials();
			loadGitHubApps();
		}
		if (v === 'previews') {
			loadPreviews();
		}
	}

	function statusVariant(
		status: string
	): 'default' | 'secondary' | 'destructive' | 'outline' | undefined {
		switch (status) {
			case 'running':
				return 'default';
			case 'failed':
				return 'destructive';
			case 'unhealthy':
				return 'destructive';
			case 'stopped':
				return 'secondary';
			default:
				return 'outline';
		}
	}

	// ── Lifecycle ──────────────────────────────────────────────────────────
	// Each concern gets its own effect so reloading the app data (which happens
	// on every save and status poll) never tears down the log stream or polling.

	// Feed panel context follows the app data and the active tab. While the Logs
	// tab streams, the feed is paused so only one socket is open per app.
	$effect(() => {
		if (!app) return;
		const ctx = appContext(app);
		setUiContext(activeTab === 'logs' ? { ...ctx, feed: { ...ctx.feed, wsUrl: null } } : ctx);
	});

	// Status polling while building (paused in hidden tabs).
	$effect(() => {
		if (appStatus !== 'building' && !statusPollRequested) return;
		return pollWhileVisible(pollStatus, 2000, { immediate: false });
	});

	// Log stream only while the Logs tab is open; restarts for a different app.
	$effect(() => {
		if (activeTab !== 'logs' || !can('logs:read')) return;
		void appId;
		untrack(startLogStream);
		return stopLogStream;
	});

	// While the app builds, link to the running deployment's live view.
	$effect(() => {
		if (appStatus !== 'building') {
			activeDeploymentId = null;
			return;
		}
		const id = appId;
		listDeployments(id)
			.then((list) => {
				if (id !== appId) return;
				activeDeploymentId =
					list.find((d) => d.status === 'running' || d.status === 'queued')?.id ?? null;
			})
			.catch(() => (activeDeploymentId = null));
	});

	// Mount-only teardown, keyed on the app id (navigating to another app
	// resets the page; a data reload does not). `?tab=` opens a tab directly
	// (the live deploy view links to where a failure is fixed).
	$effect(() => {
		void appId;
		untrack(() => {
			const requested = page.url.searchParams.get('tab');
			activeTab = requested && APP_TABS.includes(requested) ? requested : 'overview';
			handleTabChange(activeTab);
			statusPollRequested = false;
		});
		return () => {
			logBuffer.dispose();
			clearUiContext();
		};
	});
</script>

{#if app}
	<div class="space-y-6">
		<!-- Breadcrumb -->
		<Breadcrumb items={[{ label: 'Apps', href: '/apps' }, { label: app.name }]} />

		<!-- Error banner -->
		{#if error}
			<ErrorAlert title="Error" description={error} />
		{/if}

		<!-- App header -->
		<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div class="min-w-0">
				<h2 class="truncate text-3xl font-extrabold uppercase">{app.name}</h2>
				<div class="mt-2 flex min-w-0 items-center gap-3 text-sm text-muted-foreground">
					<Badge variant={statusVariant(app.status)}>{app.status}</Badge>
					<span>Source: {app.source_type}</span>
					{#if app.source_url}
						<span class="max-w-xs truncate">{app.source_url}</span>
					{/if}
				</div>
			</div>
			<div class="flex gap-2">
				{#if app.status === 'running' || app.status === 'unhealthy'}
					<Button variant="outline" size="sm" onclick={handleRestart}>Restart</Button>
					<Button variant="outline" size="sm" onclick={handleStop}>Stop</Button>
				{:else if app.status === 'stopped'}
					<Button variant="outline" size="sm" onclick={handleStart}>Start</Button>
				{/if}
				{#if activeDeploymentId}
					<Button
						variant="outline"
						size="sm"
						href="/apps/{app.id}/deployments/{activeDeploymentId}"
					>
						<SpinnerIcon class="animate-spin" /> View deploy
					</Button>
				{/if}
				<Button
					size="sm"
					onclick={handleDeploy}
					disabled={deploying || !can('apps:deploy')}
					title={can('apps:deploy') ? undefined : "Your role can't deploy apps"}
				>
					{deploying ? 'Starting…' : 'Re-deploy'}
				</Button>
				<Button variant="destructive" size="sm" onclick={() => (showDeleteDialog = true)}>
					Delete
				</Button>
			</div>
		</div>

		<Separator />

		<!-- Tabs -->
		<Tabs.Root bind:value={activeTab} onValueChange={handleTabChange}>
			<!-- 13+ tabs don't fit a phone: scroll the list, not the page -->
			<Tabs.List class="max-w-full justify-start overflow-x-auto">
				<Tabs.Trigger value="overview">Overview</Tabs.Trigger>
				<Tabs.Trigger value="logs">Logs</Tabs.Trigger>
				<Tabs.Trigger value="deployments">Deployments</Tabs.Trigger>
				<Tabs.Trigger value="env">Environment</Tabs.Trigger>
				<Tabs.Trigger value="domain">Domain</Tabs.Trigger>
				<Tabs.Trigger value="databases">Databases</Tabs.Trigger>
				<Tabs.Trigger value="health">Health</Tabs.Trigger>
				<Tabs.Trigger value="volumes">Volumes</Tabs.Trigger>
				<Tabs.Trigger value="resources">Resources</Tabs.Trigger>
				<Tabs.Trigger value="git">Git</Tabs.Trigger>
				<Tabs.Trigger value="previews">Previews</Tabs.Trigger>
				<Tabs.Trigger value="metrics">Metrics</Tabs.Trigger>
				<Tabs.Trigger value="history">History</Tabs.Trigger>
			</Tabs.List>

			<!-- Overview tab -->
			<Tabs.Content value="overview">
				<AppOverview {app} />
			</Tabs.Content>

			<!-- Logs tab -->
			<Tabs.Content value="logs">
				{#if can('logs:read')}
					<AppLogs
						appName={app.name}
						{logLines}
						streaming={logStreaming}
						onConnect={startLogStream}
						onStop={stopLogStream}
						onClear={() => logBuffer.clear()}
					/>
				{:else}
					<p class="pt-4 text-sm text-muted-foreground">Your role can't read logs.</p>
				{/if}
			</Tabs.Content>

			<!-- Deployments tab -->
			<Tabs.Content value="deployments">
				<AppDeployments
					appId={app.id}
					appName={app.name}
					{deployments}
					loading={deploymentsLoading}
					error={rollbackError}
					onRollback={handleRollback}
					canRollback={can('apps:rollback')}
					onViewGeneratedDockerfile={handleViewGeneratedDockerfile}
				/>
			</Tabs.Content>

			<!-- Environment tab -->
			<Tabs.Content value="env">
				<AppEnvironment
					appId={app.id}
					sourceType={app.source_type}
					{registryCredentials}
					registryCredentialId={app.registry_credential_id}
					onRegistryCredentialChange={handleEnvRegistryCredential}
					previewEnabled={app.preview_enabled}
					onRedeploy={handleEnvRedeploy}
				/>
			</Tabs.Content>

			<!-- Domain tab -->
			<Tabs.Content value="domain">
				<AppDomain
					{app}
					{certRefreshKey}
					onUpdate={handleUpdateDomain}
					canEdit={can('apps:write')}
				/>
			</Tabs.Content>

			<!-- Databases tab: what this app can reach (project + links) -->
			<Tabs.Content value="databases">
				<AppDatabases
					appId={app.id}
					appName={app.name}
					sourceType={app.source_type}
					projectId={app.project_id}
				/>
			</Tabs.Content>

			<!-- Health tab (port, health check, deploy timeout) -->
			<Tabs.Content value="health" class="space-y-6">
				<div class="pt-4">
					<AppPort
						{app}
						onSaved={(updated) => (app = updated)}
						onRedeploy={startDeploy}
						canEdit={can('apps:write')}
						canDeploy={can('apps:deploy')}
					/>
				</div>
				<Separator />
				<AppHealth
					{app}
					bind:healthPath
					bind:healthInterval
					bind:healthThreshold
					bind:drainTimeout
					onUpdateHealth={handleUpdateHealth}
					onDisableHealth={handleDisableHealth}
					onUpdateDrainTimeout={handleUpdateDrainTimeout}
				/>
			</Tabs.Content>

			<!-- Volumes tab -->
			<Tabs.Content value="volumes">
				<AppVolumes
					{app}
					bind:volumeContainerPath
					bind:volumeName
					canEdit={isAdmin()}
					{volumeError}
					onAdd={handleAddVolume}
					onRemove={handleRemoveVolume}
				/>
			</Tabs.Content>
			<!-- Resources tab -->
			<Tabs.Content value="resources" class="mt-4 space-y-6 pt-2">
				<div class="max-w-sm space-y-6">
					<div class="space-y-2">
						<Label>Memory Limit</Label>
						<div class="flex gap-2">
							<Input
								type="number"
								bind:value={resourceMemoryDraft}
								placeholder="Default cap"
								min={1}
								disabled={savingResources}
							/>
							<Select.Root type="single" bind:value={resourceMemoryUnit}>
								<Select.Trigger class="w-24" disabled={savingResources}>
									{resourceMemoryUnit.toUpperCase()}
								</Select.Trigger>
								<Select.Content>
									<Select.Item value="mb">MB</Select.Item>
									<Select.Item value="gb">GB</Select.Item>
								</Select.Content>
							</Select.Root>
						</div>
						<p class="text-xs text-muted-foreground">
							Leave empty for no app-specific limit; Tond still caps every container at 75% of the
							server's memory. Applies on the next deploy.
						</p>
					</div>

					<div class="space-y-2">
						<Label>CPU Limit (cores)</Label>
						<Input
							type="number"
							bind:value={resourceCpuDraft}
							placeholder="All cores"
							min={0.1}
							step={0.1}
							disabled={savingResources}
						/>
						<p class="text-xs text-muted-foreground">
							Fractional cores allowed (e.g. 0.5, 1, 2). Leave empty to let the app use every core.
						</p>
					</div>

					{#if resourcesError}
						<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive">
							{resourcesError}
						</p>
					{/if}

					<div class="flex items-center gap-3">
						<Button onclick={handleSaveResources} disabled={savingResources}>
							{savingResources ? 'Saving...' : 'Save'}
						</Button>
						{#if resourcesSaved}
							<span class="flex items-center gap-1 text-xs text-green-600 dark:text-green-400">
								<Check class="h-3.5 w-3.5" /> Saved
							</span>
						{/if}
					</div>
				</div>
			</Tabs.Content>
			<!-- Git tab -->
			<Tabs.Content value="git">
				<div class="space-y-6 pt-4">
					{#if gitError}
						<ErrorAlert title="Error" description={gitError} />
					{/if}
					{#if gitSuccess}
						<div class="rounded-md bg-success/10 p-3 text-sm text-success">
							{gitSuccess}
						</div>
					{/if}

					<!-- Source type & URL -->
					<div class="grid gap-3">
						<h3 class="text-sm font-medium">Source</h3>
						<p class="text-sm text-muted-foreground">
							How this app's deployable artifact is provided.
						</p>
						<div class="flex gap-3">
							<div class="grid gap-2">
								<Label for="source-type">Type</Label>
								<Select.Root
									type="single"
									value={sourceTypeInput}
									onValueChange={(v) => {
										if (v === 'git' || v === 'image' || v === 'tarball') sourceTypeInput = v;
									}}
								>
									<Select.Trigger id="source-type" class="w-32">
										{sourceTypeInput}
									</Select.Trigger>
									<Select.Content>
										<Select.Item value="git">git</Select.Item>
										<Select.Item value="image">image</Select.Item>
										<Select.Item value="tarball">tarball</Select.Item>
									</Select.Content>
								</Select.Root>
							</div>
							{#if sourceTypeInput !== 'tarball'}
								<div class="grid flex-1 gap-2">
									<Label for="source-url"
										>{sourceTypeInput === 'image' ? 'Docker Image' : 'Git URL'}</Label
									>
									<Input
										id="source-url"
										type="text"
										bind:value={sourceUrlInput}
										placeholder={sourceTypeInput === 'image'
											? 'registry.example.com/org/image:tag'
											: 'https://github.com/org/repo.git'}
										class="font-mono"
									/>
								</div>
							{/if}
							<Button onclick={handleUpdateSource} class="self-end">Save</Button>
						</div>

						<!-- ── GitHub App source picker ────────────────────────────────── -->
						{#if sourceTypeInput === 'git'}
							{@const installedApps = githubApps.filter((a) => a.installed)}
							{#if app?.github_app_id}
								<!-- Currently linked -->
								{@const linkedApp = githubApps.find((a) => a.id === app?.github_app_id)}
								<div class="flex items-center gap-3 rounded-md border bg-muted/20 px-4 py-3">
									<Github class="h-4 w-4 shrink-0 text-muted-foreground" />
									<div class="min-w-0 flex-1">
										<p class="text-xs font-medium">
											GitHub App: <span class="font-mono"
												>{linkedApp?.name ?? app.github_app_id}</span
											>
										</p>
										<p class="truncate font-mono text-[10px] text-muted-foreground">
											{app.source_url}
										</p>
									</div>
									<Button
										variant="outline"
										size="sm"
										class="h-7 shrink-0 text-xs text-destructive uppercase hover:text-destructive"
										disabled={githubLinkSaving}
										onclick={handleGithubUnlink}
									>
										<Unlink2 class="h-3 w-3" />
										Unlink
									</Button>
								</div>
								{#if githubLinkError}
									<p class="text-xs text-destructive">{githubLinkError}</p>
								{/if}
							{:else if installedApps.length > 0}
								<!-- Picker: choose app → repo → branch -->
								<div class="space-y-3 rounded-md border bg-muted/10 px-4 py-3">
									<p class="text-xs font-medium text-muted-foreground uppercase">
										Link a GitHub App (optional)
									</p>
									<p class="text-xs text-muted-foreground">
										Select a connected GitHub App and repository to enable private repo access and
										auto-fill the Git URL. The app will use the GitHub App's access token to clone
										the repo.
									</p>

									{#if githubLinkError}
										<p class="text-xs text-destructive">{githubLinkError}</p>
									{/if}

									<div class="flex flex-wrap gap-3">
										<!-- App selector -->
										<div class="grid gap-1.5">
											<Label class="text-xs">GitHub App</Label>
											<Select.Root
												type="single"
												value={selectedGithubAppId ?? ''}
												onValueChange={(v) => handleGithubAppChange(v || null)}
											>
												<Select.Trigger class="h-8 w-44 text-xs">
													{selectedGithubAppId
														? (installedApps.find((a) => a.id === selectedGithubAppId)?.name ??
															selectedGithubAppId)
														: 'Select app…'}
												</Select.Trigger>
												<Select.Content>
													{#each installedApps as ghApp (ghApp.id)}
														<Select.Item value={ghApp.id} class="text-xs">{ghApp.name}</Select.Item>
													{/each}
												</Select.Content>
											</Select.Root>
										</div>

										<!-- Repo selector -->
										{#if selectedGithubAppId}
											<div class="grid gap-1.5">
												<Label class="text-xs">Repository</Label>
												{#if githubPickerReposLoading}
													<p class="text-xs text-muted-foreground">Loading…</p>
												{:else}
													<Select.Root
														type="single"
														value={selectedGithubRepo?.full_name ?? ''}
														onValueChange={(v) => {
															const repo =
																githubPickerRepos[selectedGithubAppId ?? '']?.find(
																	(r) => r.full_name === v
																) ?? null;
															handleGithubRepoChange(repo);
														}}
													>
														<Select.Trigger class="h-8 w-52 font-mono text-xs">
															{selectedGithubRepo?.full_name ?? 'Select repo…'}
														</Select.Trigger>
														<Select.Content>
															{#each githubPickerRepos[selectedGithubAppId] ?? [] as repo (repo.id)}
																<Select.Item value={repo.full_name} class="font-mono text-xs">
																	{#if repo.private}
																		<Lock class="mr-1 inline h-3 w-3 text-muted-foreground" />
																	{/if}
																	{repo.full_name}
																</Select.Item>
															{/each}
														</Select.Content>
													</Select.Root>
												{/if}
											</div>
										{/if}

										<!-- Branch selector -->
										{#if selectedGithubRepo}
											{@const branchKey = `${selectedGithubAppId}/${selectedGithubRepo.full_name}`}
											<div class="grid gap-1.5">
												<Label class="text-xs">Branch</Label>
												{#if githubPickerBranchesLoading}
													<p class="text-xs text-muted-foreground">Loading…</p>
												{:else}
													<Select.Root
														type="single"
														value={selectedGithubBranch ?? ''}
														onValueChange={(v) => {
															selectedGithubBranch = v || null;
														}}
													>
														<Select.Trigger class="h-8 w-36 font-mono text-xs">
															{#if selectedGithubBranch}
																<GitBranch class="mr-1 inline h-3 w-3" />
																{selectedGithubBranch}
															{:else}
																Select branch…
															{/if}
														</Select.Trigger>
														<Select.Content>
															{#each githubPickerBranches[branchKey] ?? [] as branch (branch.name)}
																<Select.Item value={branch.name} class="font-mono text-xs">
																	{branch.name}
																	{#if branch.name === selectedGithubRepo.default_branch}
																		<span class="ml-1 text-muted-foreground">(default)</span>
																	{/if}
																</Select.Item>
															{/each}
														</Select.Content>
													</Select.Root>
												{/if}
											</div>
										{/if}

										<!-- Link button -->
										{#if selectedGithubRepo}
											<div class="grid gap-1.5">
												<Label class="invisible text-xs">Link</Label>
												<Button
													size="sm"
													class="h-8 text-xs uppercase"
													disabled={githubLinkSaving}
													onclick={handleGithubLink}
												>
													<Github class="h-3 w-3" />
													{githubLinkSaving ? 'Linking…' : 'Link Repo'}
												</Button>
											</div>
										{/if}
									</div>
								</div>
							{/if}
						{/if}
					</div>

					{#if sourceTypeInput === 'image'}
						<Separator />

						<!-- Registry Credentials -->
						<div class="grid gap-3">
							<h3 class="text-sm font-medium">Registry Credential</h3>
							<p class="text-sm text-muted-foreground">
								Attach a stored credential to authenticate pulls from a private registry.
							</p>

							{#if credError}
								<ErrorAlert title="Error" description={credError} />
							{/if}

							<!-- Attach credential dropdown -->
							<div class="flex gap-3">
								<div class="grid flex-1 gap-2">
									<Label for="reg-cred-select">Credential</Label>
									<Select.Root
										type="single"
										value={selectedCredentialId ?? '__none__'}
										onValueChange={(v) => {
											selectedCredentialId = v === '__none__' ? null : v;
										}}
									>
										<Select.Trigger id="reg-cred-select" class="w-full">
											{#if selectedCredentialId}
												{registryCredentials.find((c) => c.id === selectedCredentialId)?.name ??
													selectedCredentialId}
											{:else}
												None (public image)
											{/if}
										</Select.Trigger>
										<Select.Content>
											<Select.Item value="__none__">None (public image)</Select.Item>
											{#each registryCredentials as cred (cred.id)}
												<Select.Item value={cred.id}>{cred.name}</Select.Item>
											{/each}
										</Select.Content>
									</Select.Root>
								</div>
								<Button
									onclick={() => handleSetRegistryCredential(selectedCredentialId)}
									class="self-end"
								>
									Attach
								</Button>
							</div>

							<!-- Stored credentials list -->
							{#if registryCredentialsLoading}
								<div class="py-2 text-center text-sm text-muted-foreground">Loading...</div>
							{:else if registryCredentials.length > 0}
								<div class="divide-y rounded-md border">
									{#each registryCredentials as cred (cred.id)}
										<div class="flex items-center gap-3 px-4 py-2">
											<div class="min-w-0 flex-1">
												<span class="text-sm font-medium">{cred.name}</span>
												<span class="ml-2 font-mono text-xs text-muted-foreground"
													>{cred.server}</span
												>
												<span class="ml-2 text-xs text-muted-foreground">({cred.username})</span>
											</div>
											{#if app.registry_credential_id === cred.id}
												<span class="text-xs text-muted-foreground">attached</span>
											{/if}
											<Button
												variant="ghost"
												size="sm"
												class="h-7 px-2 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
												onclick={() => handleDeleteCredential(cred.id)}
											>
												Remove
											</Button>
										</div>
									{/each}
								</div>
							{/if}

							<!-- Add credential form -->
							{#if addCredOpen}
								<div class="grid gap-3 rounded-md border p-4">
									<h4 class="text-sm font-medium">Add credential</h4>
									{#if addCredError}
										<ErrorAlert title="Error" description={addCredError} />
									{/if}
									<div class="grid gap-2">
										<Label for="cred-name">Name</Label>
										<Input
											id="cred-name"
											type="text"
											bind:value={newCredName}
											placeholder="Docker Hub"
										/>
									</div>
									<div class="grid gap-2">
										<Label for="cred-server">Registry Server</Label>
										<Input
											id="cred-server"
											type="text"
											bind:value={newCredServer}
											placeholder="registry-1.docker.io"
											class="font-mono"
										/>
									</div>
									<div class="grid gap-2">
										<Label for="cred-username">Username</Label>
										<Input
											id="cred-username"
											type="text"
											bind:value={newCredUsername}
											placeholder="myuser"
										/>
									</div>
									<div class="grid gap-2">
										<Label for="cred-password">Password / Token</Label>
										<Input
											id="cred-password"
											type="password"
											bind:value={newCredPassword}
											placeholder="••••••••"
										/>
									</div>
									<div class="flex gap-2">
										<Button onclick={handleAddCredential}>Save Credential</Button>
										<Button
											variant="outline"
											onclick={() => {
												addCredOpen = false;
												addCredError = '';
											}}
										>
											Cancel
										</Button>
									</div>
								</div>
							{:else}
								<Button
									variant="outline"
									size="sm"
									class="self-start"
									onclick={() => (addCredOpen = true)}
								>
									Add new credential
								</Button>
							{/if}
						</div>
					{/if}

					<Separator />

					<AppWebhooks appId={app.id} />

					<Separator />

					<!-- Auto-deploy toggle -->
					<div class="grid gap-3">
						<div class="flex items-center justify-between">
							<div>
								<h3 class="text-sm font-medium">Auto-deploy</h3>
								<p class="mt-1 text-sm text-muted-foreground">
									Automatically deploy when a webhook push event is received.
								</p>
							</div>
							<Badge variant={app.auto_deploy ? 'default' : 'secondary'}>
								{app.auto_deploy ? 'Enabled' : 'Disabled'}
							</Badge>
						</div>
						<div class="flex gap-2">
							{#if !app.auto_deploy}
								<Button size="sm" onclick={() => handleSetAutoDeploy(true)}
									>Enable Auto-deploy</Button
								>
							{:else}
								<Button variant="outline" size="sm" onclick={() => handleSetAutoDeploy(false)}
									>Disable Auto-deploy</Button
								>
							{/if}
						</div>
					</div>

					<!-- Watch branch -->
					<div class="grid gap-3">
						<h3 class="text-sm font-medium">Watch Branch</h3>
						<p class="text-sm text-muted-foreground">
							Only trigger deploys for pushes to this branch. Leave empty to deploy on any branch
							push.
						</p>
						<div class="flex gap-3">
							<div class="grid flex-1 gap-2">
								<Label for="watch-branch">Branch</Label>
								<Input
									id="watch-branch"
									type="text"
									bind:value={watchBranchInput}
									placeholder="main"
									class="font-mono"
								/>
							</div>
							<Button onclick={handleSaveWatchBranch} class="self-end">Save</Button>
						</div>
						{#if app.watch_branch}
							<p class="text-xs text-muted-foreground">
								Currently watching: <span class="font-mono">{app.watch_branch}</span>
							</p>
						{:else}
							<p class="text-xs text-muted-foreground">Watching all branches.</p>
						{/if}
					</div>

					<Separator />

					<!-- Deploy key -->
					<div class="grid gap-3">
						<div class="flex items-center justify-between">
							<div>
								<h3 class="text-sm font-medium">SSH Deploy Key</h3>
								<p class="mt-1 text-sm text-muted-foreground">
									Add this Ed25519 public key to your repository's deploy keys (read-only access).
								</p>
							</div>
							<Button
								variant="outline"
								size="sm"
								onclick={handleGenerateDeployKey}
								disabled={deployKeyLoading}
							>
								{deployKey ? 'Regenerate' : 'Generate Key'}
							</Button>
						</div>

						{#if deployKeyError}
							<ErrorAlert title="Error" description={deployKeyError} />
						{/if}

						{#if deployKeyLoading}
							<div class="py-4 text-center text-sm text-muted-foreground">Loading...</div>
						{:else if deployKey}
							<SecretInput value={deployKey.public_key} readonly />
							<p class="text-xs text-muted-foreground">
								Copy this key and add it to your repository under Settings &rarr; Deploy Keys.
							</p>
						{:else}
							<div class="rounded-md border py-6 text-center text-sm text-muted-foreground">
								No deploy key generated yet. Click "Generate Key" to create one.
							</div>
						{/if}
					</div>
				</div>
			</Tabs.Content>

			<!-- Previews tab -->
			<Tabs.Content value="previews">
				<div class="space-y-6 pt-4">
					<!-- PR Preview config -->
					<div class="grid gap-3">
						<div class="flex items-center justify-between">
							<div>
								<h3 class="text-sm font-medium">PR Preview Deployments</h3>
								<p class="mt-1 text-sm text-muted-foreground">
									Automatically deploy a preview environment for every pull request opened on the
									linked GitHub repository.
								</p>
							</div>
							<Badge variant={app.preview_enabled ? 'default' : 'secondary'}>
								{app.preview_enabled ? 'Enabled' : 'Disabled'}
							</Badge>
						</div>

						{#if previewsBlocked}
							<PreviewDomainRequiredAlert />
						{/if}

						{#if previewConfigError}
							<ErrorAlert title="Error" description={previewConfigError} />
						{/if}

						<div class="flex gap-2">
							{#if !app.preview_enabled}
								<Button
									size="sm"
									disabled={previewConfigSaving || previewsBlocked}
									onclick={() => handleTogglePreviewEnabled(true)}
								>
									{previewConfigSaving ? 'Saving…' : 'Enable PR Previews'}
								</Button>
							{:else}
								<Button
									variant="outline"
									size="sm"
									disabled={previewConfigSaving}
									onclick={() => handleTogglePreviewEnabled(false)}
								>
									{previewConfigSaving ? 'Saving…' : 'Disable PR Previews'}
								</Button>
							{/if}
						</div>

						{#if !app.github_app_id}
							<p class="text-xs text-muted-foreground">
								A GitHub App must be linked (Git tab) before PR previews can be triggered
								automatically.
							</p>
						{/if}

						{#if previewDomain}
							<div class="rounded-md border border-dashed px-4 py-3 text-sm text-muted-foreground">
								Previews are served at
								<code class="font-mono text-xs">&lt;id&gt;-{app.name}.{previewDomain}</code>.
								<strong class="text-foreground">Wildcard DNS required</strong> — add a
								<code class="font-mono text-xs">*.{previewDomain} → &lt;server IP&gt;</code> A record
								so preview hostnames resolve automatically.
							</div>
						{/if}
					</div>

					<Separator />

					<AppPreviews
						{previews}
						loading={previewsLoading}
						error={previewsError}
						onDelete={handleDeletePreview}
					/>

					<!-- Load open PRs section (only when preview enabled and GitHub App linked) -->
					{#if app.preview_enabled && app.github_app_id}
						<div class="grid gap-3">
							<div class="flex items-center justify-between">
								<div>
									<h3 class="text-sm font-medium">Open Pull Requests</h3>
									<p class="text-sm text-muted-foreground">
										Load open PRs from the linked repository and deploy previews for them.
									</p>
								</div>
								<Button
									variant="outline"
									size="sm"
									disabled={openPRsLoading}
									onclick={handleLoadOpenPRs}
								>
									{openPRsLoading ? 'Loading…' : 'Load open PRs'}
								</Button>
							</div>

							{#if openPRsError}
								<p class="text-xs text-destructive">{openPRsError}</p>
							{/if}

							{#if openPRs.length > 0}
								<div class="overflow-hidden rounded border border-border">
									<table class="w-full text-sm">
										<thead>
											<tr class="border-b border-border bg-muted/50">
												<th
													class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
													>#</th
												>
												<th
													class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
													>Title</th
												>
												<th
													class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
													>Branch</th
												>
												<th class="w-36 px-3 py-2"></th>
											</tr>
										</thead>
										<tbody>
											{#each openPRs as pr, i (pr.number)}
												{@const alreadyDeployed = previews.some((p) => p.pr_number === pr.number)}
												<tr class={i < openPRs.length - 1 ? 'border-b border-border' : ''}>
													<td class="px-3 py-2 font-mono text-xs text-muted-foreground"
														>#{pr.number}</td
													>
													<td class="px-3 py-2 text-xs">
														<a
															href={safeUrl(pr.pr_url)}
															target="_blank"
															rel="noopener noreferrer"
															class="hover:underline">{pr.title}</a
														>
													</td>
													<td class="px-3 py-2 font-mono text-xs text-muted-foreground"
														>{pr.head_ref}</td
													>
													<td class="px-3 py-2">
														<div class="flex items-center justify-end gap-2">
															{#if alreadyDeployed}
																<Badge variant="secondary" class="text-xs">Deployed</Badge>
															{/if}
															<Button
																size="sm"
																variant="outline"
																disabled={deployingPR === pr.number}
																onclick={() => handleDeployPR(pr)}
															>
																{deployingPR === pr.number ? 'Deploying…' : 'Deploy Preview'}
															</Button>
														</div>
													</td>
												</tr>
											{/each}
										</tbody>
									</table>
								</div>
							{/if}
						</div>
					{/if}

					<!-- Create preview form -->
					<div class="grid gap-3">
						<h3 class="text-sm font-medium">New Preview Deployment</h3>
						<p class="text-sm text-muted-foreground">
							Creates a temporary deployment from the current image with a unique subdomain. Useful
							for PR review workflows.
						</p>
						<div class="flex items-end gap-3">
							<div class="grid gap-2">
								<Label for="preview-ttl">TTL (hours)</Label>
								<Input
									id="preview-ttl"
									type="number"
									bind:value={previewTtl}
									min={1}
									max={168}
									class="w-28"
								/>
							</div>
							<Button onclick={() => handleCreatePreview()} disabled={previewsBlocked}>
								Create Preview
							</Button>
						</div>
						<p class="text-xs text-muted-foreground">
							The preview will be automatically removed after the TTL expires.
						</p>
					</div>
				</div>
			</Tabs.Content>

			<!-- Metrics tab -->
			<Tabs.Content value="metrics">
				{#if activeTab === 'metrics'}
					{#key appId}
						<MetricsPanel owner="apps" id={appId} noun="app" />
					{/key}
				{/if}
			</Tabs.Content>

			<!-- History tab -->
			<Tabs.Content value="history">
				<AppHistory appName={app.name} />
			</Tabs.Content>
		</Tabs.Root>
	</div>
{:else}
	<div class="py-12 text-center text-muted-foreground">App not found</div>
{/if}

<!-- Delete confirmation dialog -->
<ConfirmDeleteDialog
	bind:open={showDeleteDialog}
	title={`Delete "${app?.name}"?`}
	description="This cannot be undone. The app and its container will be permanently removed."
	confirmation={app?.name ?? ''}
	onconfirm={handleDelete}
/>
