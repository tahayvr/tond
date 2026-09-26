// URL-backed tabs: the active tab lives in `?tab=` (and `?section=` for the
// app page's Settings), so every view is deep-linkable, survives reloads and
// works with back/forward, middle-click and "open in new tab".

export interface TabDef<T extends string = string> {
	id: T;
	label: string;
	/** Scope needed to see this tab; tabs the user can't use are hidden. */
	scope?: string;
}

/** Tabs the user may see, given a scope check such as `can` from auth.svelte. */
export function allowedTabs<T extends string>(
	tabs: readonly TabDef<T>[],
	can: (scope: string) => boolean
): TabDef<T>[] {
	return tabs.filter((t) => !t.scope || can(t.scope));
}

/** Read `?<param>=` and fall back when it's missing or unknown. */
export function tabFromUrl<T extends string>(
	url: URL,
	tabs: readonly TabDef<T>[],
	fallback: T,
	param = 'tab'
): T {
	const value = url.searchParams.get(param);
	return (tabs.find((t) => t.id === value)?.id ?? fallback) as T;
}

/** Same path, with `?tab=` set (the default tab drops the parameter). Other params are dropped. */
export function tabHref(
	pathname: string,
	tab: string,
	fallback: string,
	extra: Record<string, string> = {}
): string {
	const params = new URLSearchParams();
	if (tab !== fallback) params.set('tab', tab);
	for (const [key, value] of Object.entries(extra)) params.set(key, value);
	const query = params.toString();
	return query ? `${pathname}?${query}` : pathname;
}

// ── App page ────────────────────────────────────────────────────────────────

export type AppTab = 'overview' | 'deployments' | 'logs' | 'metrics' | 'settings';
export type AppSettingsSection =
	| 'environment'
	| 'domain'
	| 'databases'
	| 'health'
	| 'volumes'
	| 'resources'
	| 'git'
	| 'previews'
	| 'history'
	| 'danger';

export const APP_TABS: readonly TabDef<AppTab>[] = [
	{ id: 'overview', label: 'Overview' },
	{ id: 'deployments', label: 'Deployments' },
	{ id: 'logs', label: 'Logs', scope: 'logs:read' },
	{ id: 'metrics', label: 'Metrics', scope: 'metrics:read' },
	{ id: 'settings', label: 'Settings' }
];

export const APP_SETTINGS_SECTIONS: readonly TabDef<AppSettingsSection>[] = [
	{ id: 'environment', label: 'Environment', scope: 'env:read-names' },
	{ id: 'domain', label: 'Domain' },
	{ id: 'databases', label: 'Databases', scope: 'db:read' },
	{ id: 'health', label: 'Health' },
	{ id: 'volumes', label: 'Volumes' },
	{ id: 'resources', label: 'Resources' },
	{ id: 'git', label: 'Git' },
	{ id: 'previews', label: 'Previews' },
	{ id: 'history', label: 'History' },
	{ id: 'danger', label: 'Danger zone', scope: 'apps:delete' }
];

/**
 * Old flat tab names (and the sub-section names) still resolve, so links like
 * `?tab=domain` open Settings → Domain.
 */
export function appViewFromUrl(url: URL): { tab: AppTab; section: AppSettingsSection } {
	const raw = url.searchParams.get('tab');
	const legacy = raw === 'env' ? 'environment' : raw;
	const asSection = APP_SETTINGS_SECTIONS.find((s) => s.id === legacy);
	if (asSection) return { tab: 'settings', section: asSection.id };
	const tab = tabFromUrl(url, APP_TABS, 'overview');
	const section = tabFromUrl(url, APP_SETTINGS_SECTIONS, 'environment', 'section');
	return { tab, section };
}

export function appTabHref(appId: string, tab: AppTab, section?: AppSettingsSection): string {
	const extra: Record<string, string> =
		tab === 'settings' && section && section !== 'environment' ? { section } : {};
	return tabHref(`/apps/${appId}`, tab, 'overview', extra);
}

/** The app's Logs tab shows container output or the access log (`?log=access`). */
export type AppLogView = 'output' | 'access';
export const APP_LOG_VIEWS: readonly TabDef<AppLogView>[] = [
	{ id: 'output', label: 'App output' },
	{ id: 'access', label: 'Access log' }
];

export function appLogViewFromUrl(url: URL): AppLogView {
	return tabFromUrl(url, APP_LOG_VIEWS, 'output', 'log');
}

export function appLogViewHref(appId: string, view: AppLogView): string {
	return tabHref(`/apps/${appId}`, 'logs', 'overview', view === 'access' ? { log: 'access' } : {});
}

// ── Database and cron pages ─────────────────────────────────────────────────

export type DatabaseTab = 'overview' | 'logs' | 'environment' | 'backups' | 'metrics' | 'settings';
export const DATABASE_TABS: readonly TabDef<DatabaseTab>[] = [
	{ id: 'overview', label: 'Overview' },
	{ id: 'logs', label: 'Logs', scope: 'logs:read' },
	{ id: 'environment', label: 'Environment' },
	{ id: 'backups', label: 'Backups', scope: 'snapshots:read' },
	{ id: 'metrics', label: 'Metrics', scope: 'metrics:read' },
	{ id: 'settings', label: 'Settings' }
];

export type CronTab = 'overview' | 'runs' | 'metrics' | 'settings';
export const CRON_TABS: readonly TabDef<CronTab>[] = [
	{ id: 'overview', label: 'Overview' },
	{ id: 'runs', label: 'Runs' },
	{ id: 'metrics', label: 'Metrics', scope: 'metrics:read' },
	{ id: 'settings', label: 'Settings' }
];

export type AlertsPageTab = 'alerts' | 'rules' | 'settings';
export const ALERTS_PAGE_TABS: readonly TabDef<AlertsPageTab>[] = [
	{ id: 'alerts', label: 'Alerts' },
	{ id: 'rules', label: 'Rules' },
	{ id: 'settings', label: 'Quiet hours' }
];

export type AlertTab = 'overview' | 'settings';
export const ALERT_TABS: readonly TabDef<AlertTab>[] = [
	{ id: 'overview', label: 'Overview' },
	{ id: 'settings', label: 'Settings', scope: 'settings:write' }
];

export type ProfileTab = 'password' | '2fa' | 'sessions' | 'tokens';
export const PROFILE_TABS: readonly TabDef<ProfileTab>[] = [
	{ id: 'password', label: 'Password' },
	{ id: '2fa', label: 'Two-factor' },
	{ id: 'sessions', label: 'Sessions' },
	{ id: 'tokens', label: 'API tokens' }
];
